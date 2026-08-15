//! Managed `llama-server` sidecar lifecycle and router-mode preset file.
//!
//! The sidecar runs as a router (`--models-preset`) on a loopback random
//! port with a per-process API key, so the plugin can load, unload, and serve
//! multiple models through one process. The child is killed when the plugin
//! exits or when host configuration changes force a restart.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use ene_plugin::PluginError;
use tokio::process::Command;

use crate::config::{HostConfig, Profile, n_gpu_layers_for};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Timeout for the reuse liveness probe and each startup poll.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
static SIDECAR_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct SidecarState {
    child: Mutex<Option<tokio::process::Child>>,
    work_dir: PathBuf,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
}

impl SidecarState {
    /// Kills the sidecar child, leaving it to the OS to reap. Idempotent.
    pub(crate) fn kill(&self) {
        let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        // Killing is synchronous (`start_kill`) because the sidecar may
        // outlive the plugin process, whose runtime is already tearing down
        // when `Drop` runs — no async wait is possible there. The OS
        // reparents the child to init, which reaps it once it exits.
        if let Err(e) = child.start_kill() {
            tracing::warn!(
                component = "LlamaServerSidecar",
                error = %e,
                "Failed to kill spawned llama-server"
            );
        }
    }
}

impl Drop for SidecarState {
    fn drop(&mut self) {
        self.kill();
        if let Err(e) = std::fs::remove_dir_all(&self.work_dir) {
            tracing::warn!(
                component = "LlamaServerSidecar",
                path = %self.work_dir.display(),
                error = %e,
                "Failed to remove sidecar work directory"
            );
        }
    }
}

/// Process-wide sidecar, replaced on config change or death.
static SIDECAR: LazyLock<Mutex<Option<Arc<SidecarState>>>> = LazyLock::new(|| Mutex::new(None));
/// Serializes sidecar spawns so concurrent requests cannot start two.
static SPAWN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ensures a healthy sidecar exists for the current config and profiles,
/// spawning one when absent or unreachable.
///
/// The spawn lock is held across the whole startup wait so two concurrent
/// requests cannot spawn two sidecars.
///
/// # Errors
///
/// Returns a provider error when the `llama-server` binary cannot be
/// resolved or spawned, or never answers `/health` within the configured
/// startup timeout (the spawned child is killed in the last case).
pub(crate) async fn ensure_sidecar(
    config: &HostConfig,
    profiles: &HashMap<String, Profile>,
) -> Result<Arc<SidecarState>, PluginError> {
    if let Some(state) = current_sidecar()
        && probe_health(&state.base_url).await
    {
        return Ok(state);
    }
    let _spawn_guard = SPAWN_LOCK.lock().await;
    if let Some(state) = current_sidecar()
        && probe_health(&state.base_url).await
    {
        return Ok(state);
    }
    reset_sidecar();

    let work_dir = sidecar_work_dir();
    drop(std::fs::remove_dir_all(&work_dir));
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| PluginError::provider(format!("create sidecar work dir: {e}")))?;
    let presets_path = write_presets(&work_dir, profiles, config)?;

    let port = pick_free_port()?;
    let api_key = random_api_key();
    let binary = resolve_server_path(config)?;
    let mut child = Command::new(&binary)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--api-key",
            &api_key,
            "--no-ui",
            "--embeddings",
            "--models-preset",
            presets_path.to_str().ok_or_else(|| {
                PluginError::provider("sidecar preset path is not valid UTF-8".to_string())
            })?,
        ])
        .args(&config.server_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            PluginError::provider(format!(
                "failed to spawn llama-server '{}': {e}",
                binary.display()
            ))
        })?;

    let base_url = format!("http://127.0.0.1:{port}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.startup_timeout_secs());
    loop {
        if probe_health(&base_url).await {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            drop(child.start_kill());
            // Reap the failed child now; the plugin process is long-lived,
            // so an unreaped zombie would outlive the failed startup attempt.
            drop(child.wait().await);
            drop(std::fs::remove_dir_all(&work_dir));
            return Err(PluginError::provider(format!(
                "llama-server did not answer /health within {} s",
                config.startup_timeout_secs()
            )));
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }

    tracing::info!(
        component = "LlamaServerSidecar",
        binary = %binary.display(),
        base_url = %base_url,
        "llama-server started"
    );
    let state = Arc::new(SidecarState {
        child: Mutex::new(Some(child)),
        work_dir,
        base_url,
        api_key,
    });
    *SIDECAR.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&state));
    Ok(state)
}

/// Kills the current sidecar; the next request respawns it with fresh
/// presets. Called when host config or profiles change, and when a request
/// hits a dead sidecar.
pub(crate) fn reset_sidecar() {
    if let Some(stale) = SIDECAR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        tracing::info!(
            component = "LlamaServerSidecar",
            base_url = %stale.base_url,
            "restarting llama-server sidecar"
        );
        stale.kill();
    }
}

/// Returns the current sidecar state without spawning, if one exists.
pub(crate) fn current_sidecar() -> Option<Arc<SidecarState>> {
    SIDECAR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// True when the sidecar answers any HTTP status within the probe timeout —
/// any response proves the process is up (a 503 just means a model is
/// loading, which must not trigger a restart).
async fn probe_health(base_url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(HEALTH_PROBE_TIMEOUT)
        .timeout(HEALTH_PROBE_TIMEOUT)
        .build()
    else {
        return false;
    };
    tokio::time::timeout(
        HEALTH_PROBE_TIMEOUT,
        client.get(format!("{base_url}/health")).send(),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Resolves the `llama-server` executable: explicit config path, then the
/// plugin's own directory, then `PATH`.
fn resolve_server_path(config: &HostConfig) -> Result<PathBuf, PluginError> {
    if let Some(path) = config
        .server_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(PluginError::provider(format!(
                "configured llama-server path does not exist: {}",
                path.display()
            )));
        }
        return Ok(path);
    }
    let bundled = ene_config::builtin_plugins_dir().join(sidecar_binary_name());
    if bundled.is_file() {
        return Ok(bundled);
    }
    Ok(PathBuf::from(sidecar_binary_name()))
}

fn sidecar_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn pick_free_port() -> Result<u16, PluginError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| PluginError::provider(format!("bind loopback port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| PluginError::provider(format!("read bound port: {e}")))?
        .port();
    drop(listener);
    Ok(port)
}

/// Per-process random API key for the sidecar (defense in depth on loopback).
fn random_api_key() -> String {
    let counter = SIDECAR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!(
        "{}-{}-{}",
        std::process::id(),
        counter,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let hex = blake3::hash(seed.as_bytes()).to_hex().to_string();
    hex.chars().take(16).collect()
}

/// Unique per-spawn work directory holding the router preset file.
fn sidecar_work_dir() -> PathBuf {
    let counter = SIDECAR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ene-llama-server-{}-{counter}", std::process::id()))
}

/// Writes the router-mode preset INI covering every configured profile.
///
/// Preset section names are the profile keys the host routes with; each
/// section points at the model's cache path (downloaded lazily before the
/// first request, so the file may not exist at sidecar startup) and carries
/// the per-profile context / GPU settings. Embedding-capable profiles
/// (`dimensions` set) request last-token pooling, matching the in-process
/// plugin's embedding context.
fn write_presets(
    work_dir: &Path,
    profiles: &HashMap<String, Profile>,
    config: &HostConfig,
) -> Result<PathBuf, PluginError> {
    let acceleration = config.acceleration()?;
    let mmproj = resolve_mmproj_expected_path(config)?;
    let mut lines = vec![String::from("version = 1")];
    for (name, profile) in profiles {
        let model_path = profile.model_path().map_or_else(
            || {
                profile.url().map_or_else(
                    || {
                        Err(PluginError::provider(format!(
                            "profile {name:?} has no url or model_path"
                        )))
                    },
                    |url| {
                        crate::gguf::filename_from_url(url)
                            .map(|filename| crate::gguf::gguf_cache_dir().join(filename))
                            .map_err(|e| PluginError::provider(e.to_string()))
                    },
                )
            },
            |path| Ok(PathBuf::from(path)),
        )?;
        let layers = n_gpu_layers_for(acceleration, profile.gpu_layers());
        lines.push(String::new());
        lines.push(format!("[{name}]"));
        lines.push(format!("model = {}", model_path.display()));
        lines.push(format!("n-gpu-layers = {layers}"));
        lines.push(format!("c = {}", profile.context_size().max(256)));
        if let Some(mmproj) = &mmproj {
            lines.push(format!("mmproj = {}", mmproj.display()));
        }
        if profile.dimensions().is_some() {
            lines.push(String::from("pooling = last"));
        }
        lines.push(String::from("load-on-startup = false"));
    }
    let path = work_dir.join("models.ini");
    let ini = lines.join("\n");
    std::fs::write(&path, ini)
        .map_err(|e| PluginError::provider(format!("write sidecar presets: {e}")))?;
    Ok(path)
}

/// Expected on-disk mmproj path without downloading (download happens lazily
/// before the first vision request, same as the in-process plugin).
fn resolve_mmproj_expected_path(config: &HostConfig) -> Result<Option<PathBuf>, PluginError> {
    if config
        .mmproj_path
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
        && config
            .mmproj_url
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        return Ok(None);
    }
    if let Some(path) = config
        .mmproj_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return Ok(Some(PathBuf::from(path)));
    }
    let url = config
        .mmproj_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| PluginError::provider("mmproj_path is unset and mmproj_url is empty"))?;
    let filename =
        crate::gguf::filename_from_url(url).map_err(|e| PluginError::provider(e.to_string()))?;
    Ok(Some(crate::gguf::gguf_cache_dir().join(filename)))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    use crate::config::HostConfig;
    use serde::Deserialize as _;

    #[test]
    fn presets_cover_profiles_without_downloading() {
        let work = tempfile::tempdir().expect("tempdir");
        let profiles = HashMap::from([
            (
                "chat".to_string(),
                Profile::deserialize(serde_json::json!({
                    "model_path": "/data/chat.gguf",
                    "gpu_layers": "33",
                    "context_size": 4096,
                }))
                .expect("profile"),
            ),
            (
                "embed".to_string(),
                Profile::deserialize(serde_json::json!({
                    "model_path": "/data/embed.gguf",
                    "dimensions": 384,
                }))
                .expect("profile"),
            ),
        ]);
        let config = HostConfig::default();
        let path = write_presets(work.path(), &profiles, &config).expect("presets");
        let ini = std::fs::read_to_string(&path).expect("read");
        assert!(ini.contains("[chat]"));
        assert!(ini.contains("model = /data/chat.gguf"));
        assert!(ini.contains("n-gpu-layers = 999"));
        assert!(ini.contains("c = 4096"));
        assert!(ini.contains("[embed]"));
        assert!(ini.contains("pooling = last"));
        assert!(!ini.contains("load-on-startup = true"));
    }

    #[test]
    fn cpu_acceleration_pins_gpu_layers_to_zero() {
        let work = tempfile::tempdir().expect("tempdir");
        let profiles = HashMap::from([(
            "chat".to_string(),
            Profile::deserialize(serde_json::json!({
                "model_path": "/data/chat.gguf",
                "gpu_layers": "auto",
            }))
            .expect("profile"),
        )]);
        let config =
            HostConfig::deserialize(serde_json::json!({"acceleration": "cpu"})).expect("config");
        let path = write_presets(work.path(), &profiles, &config).expect("presets");
        let ini = std::fs::read_to_string(&path).expect("read");
        assert!(ini.contains("n-gpu-layers = 0"));
    }

    #[test]
    fn url_profiles_resolve_to_cache_paths() {
        let work = tempfile::tempdir().expect("tempdir");
        let profiles = HashMap::from([(
            "chat".to_string(),
            Profile::deserialize(serde_json::json!({
                "url": "https://cdn.example/models/chat.gguf",
            }))
            .expect("profile"),
        )]);
        let config = HostConfig::default();
        let path = write_presets(work.path(), &profiles, &config).expect("presets");
        let ini = std::fs::read_to_string(&path).expect("read");
        assert!(ini.contains("model = "));
        assert!(ini.contains("chat-"));
        assert!(ini.contains(".gguf"));
    }

    #[test]
    fn random_api_key_is_hex_and_stable_length() {
        let a = random_api_key();
        let b = random_api_key();
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(b.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
