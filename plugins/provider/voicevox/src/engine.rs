use std::time::Duration;

use ene_plugin::PluginError;
use tokio::sync::Mutex;

use crate::client;
use crate::config::{EngineMode, VoicevoxConfig};

/// How often managed mode re-probes `GET /version` while waiting for the
/// engine to boot.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Everything a spawned engine child depends on. `ensure_engine` compares
/// signatures: a changed key (mode, path, arguments, URL, timeout) means the
/// running child was started with stale settings and must be restarted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchKey {
    server_url: String,
    mode: EngineMode,
    server_path: Option<String>,
    server_args: Vec<String>,
    startup_timeout_secs: u64,
}

impl LaunchKey {
    /// The launch signature for a config.
    #[must_use]
    pub fn from_config(config: &VoicevoxConfig) -> Self {
        Self {
            server_url: config.server_url.clone(),
            mode: config.mode(),
            server_path: config.server_path.clone(),
            server_args: config.server_args.clone(),
            startup_timeout_secs: config.startup_timeout_secs,
        }
    }
}

/// A spawned engine child. Killing is synchronous (`start_kill`) because the
/// engine may outlive the plugin process, whose runtime is already tearing
/// down when `Drop` runs — no async wait is possible there. The OS reparents
/// the child to init, which reaps it once it exits.
pub struct EngineProcess {
    child: tokio::process::Child,
    key: LaunchKey,
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

impl EngineProcess {
    /// Synchronously requests termination. Safe to call more than once:
    /// an already-dead child is ignored.
    pub fn start_kill(&mut self) {
        if let Err(e) = self.child.start_kill() {
            tracing::warn!(
                component = "VoicevoxEngine",
                error = %e,
                "Failed to kill VOICEVOX engine"
            );
        }
    }

    /// Waits for the child to exit and reaps it (async).
    pub async fn reap(mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.start_kill();
        }
        drop(self.child.wait().await);
    }
}

/// Ensures an engine is serving `GET /version`, spawning one from
/// `config.server_path` when managed mode finds the server down.
///
/// A running child whose [`LaunchKey`] differs from `config` is stopped
/// first, so a `SetConfig` with a new `server_path` / `server_args` takes
/// effect on the next synthesis. The mutex guard is held across the whole
/// startup wait so two concurrent synthesize calls cannot spawn two engines.
///
/// # Errors
///
/// Returns a provider error when `server_path` is unset, the binary cannot
/// be spawned, or the engine never answers `/version` within
/// `startup_timeout_secs` (the spawned child is killed in the last case).
pub async fn ensure_engine(
    engine: &Mutex<Option<EngineProcess>>,
    config: &VoicevoxConfig,
) -> Result<(), PluginError> {
    let key = LaunchKey::from_config(config);
    let mut guard = engine.lock().await;
    if guard.as_ref().is_some_and(|process| process.key == key) {
        return Ok(());
    }
    // A stale child (killed synchronously by `set_config` on a launch
    // signature change, or still running from an older config) must be
    // fully reaped before the new engine can start: `engine_reachable`
    // would otherwise pick up the dying old HTTP endpoint and skip the
    // restart.
    if let Some(stale) = guard.take() {
        stale.reap().await;
    }
    if client::engine_reachable(config).await {
        return Ok(());
    }
    let path = config.server_path.as_deref().ok_or_else(|| {
        PluginError::provider("voicevox managed mode is enabled but server_path is not configured")
    })?;
    let mut child = tokio::process::Command::new(path)
        .args(&config.server_args)
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
            *guard = Some(EngineProcess { child, key });
            tracing::info!(
                component = "VoicevoxEngine",
                path = %path,
                "VOICEVOX engine started"
            );
            Ok(())
        }
        Ok(Err(e)) => {
            drop(child.start_kill());
            // Reap the failed child now; the plugin process is long-lived,
            // so an unreaped zombie would outlive the failed startup attempt.
            drop(child.wait().await);
            Err(e)
        }
        Err(_) => {
            drop(child.start_kill());
            drop(child.wait().await);
            Err(PluginError::provider(format!(
                "VOICEVOX engine did not answer GET /version within {} s",
                config.startup_timeout_secs.max(1)
            )))
        }
    }
}
