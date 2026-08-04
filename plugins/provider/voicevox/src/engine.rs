//! Managed-mode engine lifecycle: spawn, health-check, terminate.

use std::time::Duration;

use ene_plugin::PluginError;
use tokio::sync::Mutex;

use crate::client;
use crate::config::VoicevoxConfig;

/// How often managed mode re-probes `GET /version` while waiting for the
/// engine to boot.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A spawned engine child. Killing is synchronous (`start_kill`) because the
/// engine may outlive the plugin process, whose runtime is already tearing
/// down when `Drop` runs — no async wait is possible there. The OS reparents
/// the child to init, which reaps it once it exits.
pub struct EngineProcess {
    child: tokio::process::Child,
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        if let Err(e) = self.child.start_kill() {
            tracing::warn!(
                component = "VoicevoxEngine",
                error = %e,
                "Failed to kill spawned VOICEVOX engine"
            );
        }
    }
}

/// Ensures an engine is serving `GET /version`, spawning one from
/// `config.engine_path` when `auto_start` mode finds the server down.
///
/// The mutex guard is held across the whole startup wait so two concurrent
/// synthesize calls cannot spawn two engines.
///
/// # Errors
///
/// Returns a provider error when `engine_path` is unset, the binary cannot
/// be spawned, or the engine never answers `/version` within
/// `startup_timeout_secs` (the spawned child is killed in the last case).
pub async fn ensure_engine(
    engine: &Mutex<Option<EngineProcess>>,
    config: &VoicevoxConfig,
) -> Result<(), PluginError> {
    let mut guard = engine.lock().await;
    if guard.is_some() {
        return Ok(());
    }
    if client::engine_reachable(config).await {
        return Ok(());
    }
    let path = config.engine_path.as_deref().ok_or_else(|| {
        PluginError::provider("voicevox auto_start is enabled but engine_path is not configured")
    })?;
    let mut child = tokio::process::Command::new(path)
        .args(&config.engine_args)
        .spawn()
        .map_err(|e| {
            PluginError::provider(format!("failed to spawn VOICEVOX engine '{path}': {e}"))
        })?;

    let started = tokio::time::timeout(
        Duration::from_secs(config.startup_timeout_secs.max(1)),
        async {
            loop {
                if client::engine_reachable(config).await {
                    return Ok(());
                }
                tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
            }
        },
    )
    .await;
    match started {
        Ok(Ok(())) => {
            *guard = Some(EngineProcess { child });
            tracing::info!(
                component = "VoicevoxEngine",
                path = %path,
                "VOICEVOX engine started"
            );
            Ok(())
        }
        Ok(Err(e)) => {
            drop(child.start_kill());
            Err(e)
        }
        Err(_) => {
            drop(child.start_kill());
            Err(PluginError::provider(format!(
                "VOICEVOX engine did not answer GET /version within {} s",
                config.startup_timeout_secs.max(1)
            )))
        }
    }
}
