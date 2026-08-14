//! Spawn → health-check → kill lifecycle for the `__SIDECAR_NAME__` sidecar.
//!
//! Mirrors `plugins/provider/llama-server/src/server.rs` and
//! `plugins/provider/voicevox/src/engine.rs`; keep those and this template in
//! sync when the lifecycle contract changes.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;

use super::config::{SidecarConfig, SidecarProfiles};
use super::preset;

/// How often startup re-probes the health endpoint while the engine boots.
pub const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The spawned sidecar child plus connection details clients need.
pub struct SidecarState {
    child: Mutex<Option<tokio::process::Child>>,
    pub work_dir: PathBuf,
    pub base_url: String,
    pub api_key: String,
}

impl Drop for SidecarState {
    fn drop(&mut self) {
        self.kill();
    }
}

impl SidecarState {
    fn kill(&self) {
        if let Some(mut child) = self.child.lock().unwrap_or_else(|p| p.into_inner()).take() {
            if let Err(e) = child.start_kill() {
                tracing::warn!(
                    component = "__SIDECAR_UPPER__Sidecar",
                    error = %e,
                    "Failed to kill spawned sidecar"
                );
            }
        }
    }
}

static SIDECAR: LazyLock<Mutex<Option<Arc<SidecarState>>>> = LazyLock::new(|| Mutex::new(None));
static SPAWN_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));

/// Serializes spawns so concurrent requests cannot start two engines.
pub async fn ensure_sidecar(
    config: &SidecarConfig,
    profiles: &SidecarProfiles,
) -> Result<Arc<SidecarState>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(state) = current_sidecar() {
        return Ok(state);
    }
    let _spawn_guard = SPAWN_LOCK.lock().await;
    if let Some(state) = current_sidecar() {
        return Ok(state);
    }
    reset_sidecar();

    let work_dir = sidecar_work_dir();
    drop(std::fs::remove_dir_all(&work_dir));
    std::fs::create_dir_all(&work_dir)?;
    let presets_path = preset::write_presets(&work_dir, profiles)?;
    let port = pick_free_port()?;
    let api_key = random_api_key();
    let binary = resolve_binary(config)?;
    let mut child = Command::new(&binary)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--api-key",
            &api_key,
            // Engine-specific hook: the reference engine (llama-server)
            // consumes the preset file via `--models-preset`. Engines with a
            // different flag (or none) adapt this line.
            "--models-preset",
            presets_path
                .to_str()
                .ok_or_else(|| "sidecar preset path is not valid UTF-8".to_string())?,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let base_url = format!("http://127.0.0.1:{port}");
    let deadline = tokio::time::Instant::now() + config.startup_timeout();
    loop {
        if probe_health(&base_url).await {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            drop(child.start_kill());
            drop(child.wait().await);
            drop(std::fs::remove_dir_all(&work_dir));
            return Err("sidecar did not answer the health probe in time".into());
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }

    let state = Arc::new(SidecarState {
        child: Mutex::new(Some(child)),
        work_dir,
        base_url,
        api_key,
    });
    *SIDECAR.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::clone(&state));
    Ok(state)
}

/// Kills the current sidecar; the next request respawns it with fresh
/// presets. Called on config change and when a request hits a dead sidecar.
pub fn reset_sidecar() {
    if let Some(stale) = SIDECAR.lock().unwrap_or_else(|p| p.into_inner()).take() {
        tracing::info!(
            component = "__SIDECAR_UPPER__Sidecar",
            base_url = %stale.base_url,
            "restarting sidecar"
        );
        stale.kill();
    }
}

/// Returns the current sidecar state without spawning, if one exists.
pub fn current_sidecar() -> Option<Arc<SidecarState>> {
    SIDECAR.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// True when the engine answers the health probe within its timeout — any
/// response proves the process is up (a 503 just means a model is loading).
pub async fn probe_health(base_url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(1))
        .build()
    else {
        return false;
    };
    tokio::time::timeout(
        Duration::from_secs(1),
        client.get(format!("{base_url}/health")).send(),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Picks a free loopback TCP port by binding and releasing a listener.
pub fn pick_free_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Random 32-hex API key for engine authentication.
pub fn random_api_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Per-process work directory for presets and scratch files.
pub fn sidecar_work_dir() -> PathBuf {
    ene_config::app_data_dir()
        .join("sidecar-work")
        .join("__SIDECAR_NAME__")
}

/// Resolves the engine binary: explicit config `server_path` → bundled
/// plugins directory → `PATH`. Never downloads a binary; catalog-managed
/// engines arrive as a host-injected `server_path`.
pub fn resolve_binary(
    config: &SidecarConfig,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(path) = config
        .server_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(format!(
                "configured __SIDECAR_NAME__ path does not exist: {}",
                path.display()
            )
            .into());
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
        "__SIDECAR_NAME__.exe"
    } else {
        "__SIDECAR_NAME__"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_free_port_binds() {
        let port = pick_free_port().expect("port");
        assert!(port > 0);
    }

    #[test]
    fn api_key_is_hex() {
        let key = random_api_key();
        assert_eq!(key.len(), 32);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn resolve_binary_prefers_config_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let binary = dir.path().join("engine");
        std::fs::write(&binary, b"#!/bin/sh").expect("write fixture");
        let config = SidecarConfig {
            server_path: Some(binary.to_string_lossy().into_owned()),
            ..SidecarConfig::default()
        };
        assert_eq!(
            resolve_binary(&config).expect("resolves").file_name(),
            Some(std::ffi::OsStr::new("engine"))
        );
    }

    #[test]
    fn resolve_binary_rejects_missing_config_path() {
        let config = SidecarConfig {
            server_path: Some("/nonexistent/engine".to_string()),
            ..SidecarConfig::default()
        };
        assert!(resolve_binary(&config).is_err());
    }
}
