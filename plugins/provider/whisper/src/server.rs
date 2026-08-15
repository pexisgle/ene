//! Managed `whisper-server` sidecar lifecycle and transcription client.
//!
//! Follows the sidecar template (`templates/sidecar`): spawn on loopback
//! with a free port, health-check with a timeout, kill on `Drop`, and
//! respawn when the config or the model path changes. The in-process
//! whisper.cpp engine remains the fallback for `mode: auto` without a
//! configured server binary.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use ene_plugin::PluginError;
use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;

use crate::config::WhisperConfig;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) struct SidecarState {
    child: Mutex<Option<tokio::process::Child>>,
    base_url: String,
    model_path: PathBuf,
}

impl Drop for SidecarState {
    fn drop(&mut self) {
        self.kill();
    }
}

impl SidecarState {
    fn kill(&self) {
        if let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && let Err(e) = child.start_kill()
        {
            tracing::warn!(
                component = "WhisperSidecar",
                error = %e,
                "Failed to kill spawned whisper-server"
            );
        }
    }
}

static SIDECAR: LazyLock<Mutex<Option<Arc<SidecarState>>>> = LazyLock::new(|| Mutex::new(None));
static SPAWN_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));

/// Serializes sidecar spawns so concurrent requests cannot start two.
///
/// Respawns when the resolved model path differs from the running sidecar's
/// (whisper-server serves one model per process), or when the current
/// sidecar is dead.
pub(crate) async fn ensure_sidecar(
    config: &WhisperConfig,
    model_path: &Path,
) -> Result<Arc<SidecarState>, PluginError> {
    if let Some(state) = current_sidecar() {
        if state.model_path == model_path && probe_health(&state.base_url).await {
            return Ok(state);
        }
        reset_sidecar();
    }
    let _spawn_guard = SPAWN_LOCK.lock().await;
    if let Some(state) = current_sidecar()
        && state.model_path == model_path
        && probe_health(&state.base_url).await
    {
        return Ok(state);
    }
    reset_sidecar();

    let port = pick_free_port()?;
    let binary = resolve_server_path(config)?;
    let mut child = Command::new(&binary)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--model",
            model_path.to_str().ok_or_else(|| {
                PluginError::provider(format!(
                    "whisper model path is not valid UTF-8: {}",
                    model_path.display()
                ))
            })?,
        ])
        .args(&config.server_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            PluginError::provider(format!(
                "failed to spawn whisper-server '{}': {e}",
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
            return Err(PluginError::provider(format!(
                "whisper-server did not answer /health within {} s",
                config.startup_timeout_secs()
            )));
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }

    tracing::info!(
        component = "WhisperSidecar",
        binary = %binary.display(),
        model = %model_path.display(),
        base_url = %base_url,
        "whisper-server started"
    );
    let state = Arc::new(SidecarState {
        child: Mutex::new(Some(child)),
        base_url,
        model_path: model_path.to_path_buf(),
    });
    *SIDECAR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&state));
    Ok(state)
}

/// Kills the current sidecar; the next request respawns it with fresh args.
/// Called when the host config changes and when a request hits a dead
/// sidecar or a different model path.
pub(crate) fn reset_sidecar() {
    if let Some(stale) = SIDECAR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        tracing::info!(
            component = "WhisperSidecar",
            base_url = %stale.base_url,
            "restarting whisper-server sidecar"
        );
        stale.kill();
    }
}

fn current_sidecar() -> Option<Arc<SidecarState>> {
    SIDECAR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Transcribes WAV bytes through the sidecar's OpenAI-compatible endpoint.
pub(crate) async fn transcribe(
    state: &SidecarState,
    wav_bytes: Vec<u8>,
    language: Option<&str>,
) -> Result<ene_ai::SttResult, PluginError> {
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(wav_bytes)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| PluginError::provider(format!("invalid mime: {e}")))?,
        )
        .text("model", "whisper-1")
        .text("response_format", "json");
    let form = match language {
        Some(language) if !language.is_empty() => form.text("language", language.to_string()),
        _ => form,
    };
    let client = http_client()?;
    let response = client
        .post(format!("{}/v1/audio/transcriptions", state.base_url))
        .multipart(form)
        .send()
        .await
        .map_err(|e| PluginError::provider(format!("whisper-server request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(PluginError::provider(format!(
            "whisper-server transcription failed: {}",
            response.status()
        )));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| PluginError::provider(format!("whisper-server response is not JSON: {e}")))?;
    let text = body
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            PluginError::provider(format!("whisper-server response missing 'text': {body}"))
        })?
        .trim()
        .to_string();
    Ok(ene_ai::SttResult {
        text,
        language: None,
        duration_secs: 0.0,
    })
}

/// Shared loopback-only HTTP client (no TLS needed; ring provider installed
/// once so reqwest's rustls backend has a default).
fn http_client() -> Result<reqwest::Client, PluginError> {
    drop(rustls::crypto::ring::default_provider().install_default());
    reqwest::Client::builder()
        .timeout(Duration::from_mins(2))
        .build()
        .map_err(|e| PluginError::provider(format!("HTTP client init failed: {e}")))
}

/// Resolves the `whisper-server` executable: explicit config path, then the
/// bundled plugins directory, then `PATH`.
fn resolve_server_path(config: &WhisperConfig) -> Result<PathBuf, PluginError> {
    if let Some(path) = config
        .server_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(PluginError::provider(format!(
                "configured whisper-server path does not exist: {}",
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
        "whisper-server.exe"
    } else {
        "whisper-server"
    }
}

fn pick_free_port() -> Result<u16, PluginError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| PluginError::provider(format!("bind loopback port: {e}")))?;
    Ok(listener
        .local_addr()
        .map_err(|e| PluginError::provider(format!("read loopback port: {e}")))?
        .port())
}

/// True when the sidecar answers the health probe — any response proves the
/// process is up (a 503 just means a model is loading, which must not
/// trigger a restart).
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

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn free_port_binds() {
        let port = pick_free_port().expect("port");
        assert!(port > 0);
    }

    #[test]
    fn binary_name_is_platform_consistent() {
        let name = sidecar_binary_name();
        assert!(!name.is_empty());
    }
}
