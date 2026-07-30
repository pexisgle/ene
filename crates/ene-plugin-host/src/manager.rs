//! Plugin host manager: process supervision, capability routing, and lifecycle.
//!
//! [`PluginHostManager`] starts only plugins explicitly listed in
//! `plugins.list` with `enable: true` (opt-in discovery). Each plugin is
//! spawned with a hardened environment (`env_clear()` + whitelist), performs
//! the v3 handshake, and routes advertised capabilities (tools, LLM
//! providers) into the appropriate host registries. It also connects to
//! configured MCP servers and exposes their tools alongside plugin-provided
//! tools.

use std::collections::HashMap;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ene_config::EneConfig;
use tokio::sync::{Mutex, mpsc};

use sha2::Digest;

use ene_connector::ConnectorRegistry;

use crate::circuit_breaker::CircuitBreaker;
use crate::error::PluginHostError;
use crate::factory::IpcLlmProviderFactory;
use crate::health::PluginHealthEvent;
use crate::ipc_plugin::IpcPluginConnection;
use crate::mcp_config::McpTransport;
use crate::mcp_connector::McpConnector;
use crate::tool_registry::{DeferredCallResult, ToolRegistry};

/// Maximum number of restart attempts before a plugin is disabled.
const MAX_RESTARTS: usize = 5;
/// Base delay for exponential backoff between restarts.
const BASE_DELAY_MS: u64 = 500;
/// Maximum delay cap for exponential backoff.
const MAX_DELAY_MS: u64 = 30_000;

/// The checksum a plugin binary is pinned to for restart-time verification.
///
/// Both variants are verified identically on every restart; the distinction
/// is purely documentary — it records *how* the checksum came to be pinned so
/// the non-recoverability of a mismatch is self-explanatory at the type level.
/// There is deliberately no "no checksum" variant: a supervised plugin always
/// verifies its binary on restart (no fail-open path, #429).
enum PinnedChecksum {
    /// User explicitly configured this checksum — mismatch is a hard fail.
    Configured(String),
    /// Trust-on-first-use: recorded at startup. Mismatch means the binary
    /// was replaced while the host was running (e.g. cargo build in dev).
    /// Still a hard fail for this process lifetime (the running instance
    /// was verified against the original), but documented as expected.
    Tofu(String),
}

impl PinnedChecksum {
    /// The pinned checksum as a hex string, regardless of how it was pinned.
    fn hex(&self) -> &str {
        match self {
            Self::Configured(hex) | Self::Tofu(hex) => hex,
        }
    }
}

/// A supervised plugin process and its IPC connection.
struct SupervisedPlugin {
    name: String,
    child: std::process::Child,
    socket_path: PathBuf,
    binary_path: PathBuf,
    /// The checksum the binary is pinned to, re-verified on every restart.
    /// Always present for plugins started via `start_plugin`: either the
    /// user-configured checksum ([`PinnedChecksum::Configured`]) or the
    /// trust-on-first-use checksum recorded at startup
    /// ([`PinnedChecksum::Tofu`]). Restart-time verification is therefore
    /// always active — the binary pinned at startup must still be on disk to
    /// restart. There is no "no checksum" state, so no fail-open path (#429).
    pinned_checksum: PinnedChecksum,
    sandbox: ene_plugin_proto::SandboxConfigData,
    plugin_config: Option<serde_json::Value>,
    /// Environment variable names to copy from the host on restart.
    env_passthrough: Vec<String>,
    restart_count: usize,
    /// Set once the plugin is permanently disabled (restart budget exhausted
    /// or a restart-time checksum mismatch). The health probe loop skips
    /// disabled plugins so it does not keep restarting or re-emitting
    /// `PluginHealthEvent::Disabled` for them.
    disabled: bool,
}

impl SupervisedPlugin {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn delay_for_restart(&self) -> Duration {
        delay_for_restart(self.restart_count)
    }

    fn restart(&mut self) -> Result<(), PluginHostError> {
        self.restart_count = self.restart_count.saturating_add(1);
        if self.restart_count > MAX_RESTARTS {
            return Err(PluginHostError::ExecutionFailed {
                message: format!(
                    "Plugin '{}' exceeded max restarts ({})",
                    self.name, MAX_RESTARTS
                ),
            });
        }

        // Verify the binary checksum BEFORE killing the running process or
        // spawning a replacement. At this point the process is already dead
        // or unresponsive (that's why the health probe triggered a restart).
        // The goal of verifying BEFORE kill/spawn is to avoid exec'ing a
        // tampered binary — not to preserve a "good" process.
        //
        // On a mismatch we still reap the dead/hung child (kill + wait) and
        // remove its socket before returning the error, so we don't leave a
        // zombie process or a stale socket behind. The mismatch is surfaced
        // by the caller as a `PluginHealthEvent::Disabled` diagnostic (#429).
        //
        // Residual window: `spawn()` below re-opens the binary by path, so
        // an attacker with write access to the plugin directory could swap
        // the file between this check and the exec. Closing that would
        // require spawning from an already-opened file descriptor, which
        // std does not support cross-platform (Windows, and macOS has no
        // /proc); such an attacker could also swap the binary immediately
        // after any spawn. Checksums here are tamper *detection* for a
        // binary replaced while the host runs, not a guarantee against an
        // active adversary.
        if let Err(e) =
            verify_plugin_checksum(&self.name, &self.binary_path, self.pinned_checksum.hex())
        {
            // Reap the dead/hung child and clean up the socket before
            // surfacing the mismatch, avoiding zombies and stale sockets.
            drop(self.child.kill());
            drop(self.child.wait());
            ene_plugin_proto::cleanup_path(&self.socket_path);
            return Err(e);
        }

        drop(self.child.kill());
        drop(self.child.wait());

        ene_plugin_proto::cleanup_path(&self.socket_path);

        tracing::warn!(
            component = "PluginHostManager",
            plugin = %self.name,
            attempt = self.restart_count,
            max = MAX_RESTARTS,
            "Restarting plugin"
        );

        let child =
            build_plugin_command(&self.binary_path, &self.socket_path, &self.env_passthrough)
                .spawn()
                .map_err(|e| PluginHostError::SpawnFailed {
                    name: self.name.clone(),
                    reason: e.to_string(),
                })?;

        self.child = child;
        Ok(())
    }
}

impl Drop for SupervisedPlugin {
    fn drop(&mut self) {
        tracing::info!(
            component = "PluginHostManager",
            plugin = %self.name,
            "Stopping plugin"
        );
        // Kill the child and reap it to avoid leaving a zombie process.
        // Both are best-effort: the process may already have exited.
        drop(self.child.kill());
        drop(self.child.wait());
        ene_plugin_proto::cleanup_path(&self.socket_path);
    }
}

/// Pure-function form of the per-restart backoff delay.
fn delay_for_restart(restart_count: usize) -> Duration {
    let delay_ms = BASE_DELAY_MS.saturating_mul(2u64.saturating_pow(restart_count as u32));
    Duration::from_millis(delay_ms.min(MAX_DELAY_MS))
}

/// Environment variable names that must never be forwarded via
/// `env_passthrough`, regardless of user configuration. These could
/// subvert the sandbox or hijack the IPC channel.
const ENV_PASSTHROUGH_DENYLIST: &[&str] = &[
    "LD_PRELOAD",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_FORCE_FLAT_NAMESPACE",
    "ENE_PLUGIN_SOCKET",
];

/// Abstraction over command types that support environment manipulation.
///
/// Both `std::process::Command` and `tokio::process::Command` implement
/// this, allowing the env-hardening logic to be shared between the plugin
/// spawn path (std) and the MCP stdio spawn path (tokio).
pub(crate) trait EnvCommand {
    /// Clears the entire inherited environment.
    fn clear_env(&mut self);
    /// Sets a single environment variable.
    fn set_env(&mut self, key: &str, val: &str);
}

impl EnvCommand for std::process::Command {
    fn clear_env(&mut self) {
        self.env_clear();
    }
    fn set_env(&mut self, key: &str, val: &str) {
        self.env(key, val);
    }
}

impl EnvCommand for tokio::process::Command {
    fn clear_env(&mut self) {
        self.env_clear();
    }
    fn set_env(&mut self, key: &str, val: &str) {
        self.env(key, val);
    }
}

/// Applies the hardened environment to a command.
///
/// Clears the inherited environment (`env_clear()`) and forwards only an
/// explicit whitelist of essential platform variables, plus any
/// caller-supplied `env_passthrough` entries (filtered against a denylist).
///
/// This is the single source of truth for env hardening — used by both
/// the plugin spawn path and the MCP stdio spawn path.
pub(crate) fn apply_hardened_env(cmd: &mut impl EnvCommand, env_passthrough: &[String]) {
    cmd.clear_env();

    // Essential platform variables (Unix + general).
    for var in ["PATH", "HOME", "TMPDIR", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            cmd.set_env(var, &val);
        }
    }

    // Timezone: forward only when set.
    if let Ok(tz) = std::env::var("TZ") {
        cmd.set_env("TZ", &tz);
    }

    // Shared library path on Linux.
    #[cfg(target_os = "linux")]
    if let Ok(val) = std::env::var("LD_LIBRARY_PATH") {
        cmd.set_env("LD_LIBRARY_PATH", &val);
    }

    // Windows requires these for basic process operation.
    #[cfg(windows)]
    for var in ["SystemRoot", "USERPROFILE", "APPDATA", "TEMP", "PATHEXT"] {
        if let Ok(val) = std::env::var(var) {
            cmd.set_env(var, &val);
        }
    }

    // Per-plugin explicit passthrough (interim until #412/#413),
    // filtered against the denylist.
    for var in env_passthrough {
        if ENV_PASSTHROUGH_DENYLIST
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(var))
        {
            tracing::warn!(
                component = "PluginHostManager",
                variable = %var,
                "env_passthrough entry is on the denylist; ignoring"
            );
            continue;
        }
        if let Ok(val) = std::env::var(var) {
            cmd.set_env(var, &val);
        }
    }
}

/// Builds a [`std::process::Command`] for a plugin with a hardened
/// environment.
///
/// The inherited environment is cleared (`env_clear()`) and only an
/// explicit whitelist of essential variables is forwarded:
///
/// - `PATH` — locating system executables
/// - `HOME` — user config files
/// - `TMPDIR` — temporary files
/// - `LANG` — locale-sensitive output
/// - `TZ` — timezone awareness (only if set)
/// - `LD_LIBRARY_PATH` — shared library loading on Linux
/// - `SystemRoot`, `USERPROFILE`, `APPDATA`, `TEMP`, `PATHEXT` — Windows
/// - `ENE_PLUGIN_SOCKET` — the IPC channel
///
/// Additional variables can be forwarded per-plugin via `env_passthrough`,
/// subject to a denylist that blocks security-sensitive names.
fn build_plugin_command(
    binary_path: &std::path::Path,
    socket_path: &std::path::Path,
    env_passthrough: &[String],
) -> std::process::Command {
    let mut cmd = std::process::Command::new(binary_path);
    apply_hardened_env(&mut cmd, env_passthrough);

    // IPC socket — the primary communication channel.
    cmd.env("ENE_PLUGIN_SOCKET", socket_path);

    cmd
}

/// A `ToolRegistry` adapter that routes tool calls to a plugin over IPC,
/// guarded by a per-plugin circuit breaker.
struct PluginToolRegistry {
    /// Name of the plugin that owns these tools (used for health events).
    plugin_name: String,
    conn: Arc<Mutex<IpcPluginConnection>>,
    /// Handle to the supervised process, used to reset the restart budget
    /// after a successful call (restoring the old `ToolHostManager` behavior).
    supervised: Arc<Mutex<SupervisedPlugin>>,
    tools: Vec<ene_plugin_proto::ToolSpec>,
    breaker: parking_lot::Mutex<CircuitBreaker>,
    health_tx: Option<mpsc::UnboundedSender<PluginHealthEvent>>,
}

impl PluginToolRegistry {
    fn emit_health(&self, event: PluginHealthEvent) {
        if let Some(tx) = &self.health_tx {
            drop(tx.send(event));
        }
    }
}

#[async_trait]
impl ToolRegistry for PluginToolRegistry {
    fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
        self.tools.clone()
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&ene_plugin_proto::CallContext>,
    ) -> Result<ene_plugin_proto::ToolResult, PluginHostError> {
        {
            let mut breaker = self.breaker.lock();
            if breaker.is_open() {
                return Err(PluginHostError::CircuitOpen {
                    tool: name.to_string(),
                    consecutive_failures: breaker.consecutive_failures(),
                });
            }
        }

        let result = {
            let mut conn = self.conn.lock().await;
            conn.call_tool(name, arguments, context.cloned()).await
        };

        if result.is_ok() {
            {
                let mut breaker = self.breaker.lock();
                if breaker.consecutive_failures() != 0 {
                    self.emit_health(PluginHealthEvent::CircuitClosed {
                        plugin: self.plugin_name.clone(),
                    });
                }
                breaker.record_success();
            }
            // A healthy round-trip clears the restart budget so a plugin
            // that has been running well is not penalized for old crashes.
            self.supervised.lock().await.restart_count = 0;
        } else {
            let mut breaker = self.breaker.lock();
            if breaker.record_failure() {
                let failures = breaker.consecutive_failures();
                self.emit_health(PluginHealthEvent::CircuitOpened {
                    plugin: self.plugin_name.clone(),
                    consecutive_failures: failures,
                });
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin = %self.plugin_name,
                    tool = %name,
                    consecutive_failures = failures,
                    "Circuit breaker opened for plugin tool"
                );
            }
        }

        // Propagate structured `ToolError` (e.g. `PermissionRequired`,
        // `UserInputRequired`) untouched so the streaming layer can match on
        // the interactive variants. Flattening to a string here would silently
        // disable the permission / user-input contract.
        result
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&ene_plugin_proto::CallContext>,
    ) -> Result<DeferredCallResult, PluginHostError> {
        let outcome = {
            let mut conn = self.conn.lock().await;
            conn.call_tool_deferred(name, arguments, context.cloned())
                .await?
        };
        Ok(match outcome {
            ene_plugin_proto::DeferredOutcome::Sync(result) => DeferredCallResult::Sync(result),
            ene_plugin_proto::DeferredOutcome::Deferred { task_id } => {
                DeferredCallResult::Deferred { task_id }
            }
        })
    }

    async fn poll_deferred(
        &self,
        _tool_name: &str,
        task_id: &str,
    ) -> ene_plugin_proto::DeferredStatus {
        let mut conn = self.conn.lock().await;
        match conn.poll_deferred(task_id).await {
            Ok(status) => status,
            Err(e) => {
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin = %self.plugin_name,
                    task_id = %task_id,
                    error = %e,
                    "Failed to poll deferred task"
                );
                ene_plugin_proto::DeferredStatus::Unknown
            }
        }
    }

    async fn cancel_deferred(&self, _tool_name: &str, task_id: &str) {
        let mut conn = self.conn.lock().await;
        if let Err(e) = conn.cancel_deferred(task_id).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                task_id = %task_id,
                error = %e,
                "Failed to cancel deferred task"
            );
        }
    }

    async fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        let mut conn = self.conn.lock().await;
        if let Err(e) = conn.set_call_context(ctx).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                error = %e,
                "Failed to set call context"
            );
        }
    }

    async fn approve_permission(&self, request_id: &str) {
        let mut conn = self.conn.lock().await;
        if let Err(e) = conn.approve_permission(request_id).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                request_id = %request_id,
                error = %e,
                "Failed to approve permission"
            );
        }
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let mut conn = self.conn.lock().await;
        if let Err(e) = conn.allow_pattern(action, target_pattern).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                action = %action,
                error = %e,
                "Failed to allow pattern"
            );
        }
    }

    async fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        let mut conn = self.conn.lock().await;
        if let Err(e) = conn.revoke_pattern(action, target_pattern).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                action = %action,
                error = %e,
                "Failed to revoke pattern"
            );
        }
    }
}

/// Orchestrates the lifecycle of all plugin processes and MCP connections.
///
/// Starts only plugins explicitly listed in `plugins.list` with
/// `enable: true` (opt-in discovery). Binaries found on disk that are
/// not listed emit a warning suggesting the user add them. Each plugin
/// is spawned with a hardened environment (`env_clear()` + whitelist),
/// performs the v3 handshake, and routes capabilities:
///
/// - `capabilities.tools` (count) → wrapped in a [`ToolRegistry`] adapter (specs fetched via `ListTools`)
/// - `capabilities.llm_providers` → registered as [`IpcLlmProviderFactory`] entries
///
/// Additionally connects to any MCP servers declared in `plugins.mcp_servers`
/// and includes their tools in [`tool_registries`](Self::tool_registries).
pub struct PluginHostManager {
    supervised: Vec<Arc<Mutex<SupervisedPlugin>>>,
    connections: Vec<Arc<Mutex<IpcPluginConnection>>>,
    tool_registries: Vec<Arc<dyn ToolRegistry>>,
    llm_factories: HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>>,
    health_task: Option<tokio::task::JoinHandle<()>>,
    health_rx: Option<mpsc::UnboundedReceiver<PluginHealthEvent>>,
    /// When true (default), Drop will attempt a best-effort graceful shutdown
    /// by killing child processes with a brief wait for reaping.
    shutdown_on_drop: bool,
    /// Connector registry for MCP servers (`WS7`: wire `McpConnector` into lifecycle).
    connector_registry: Arc<ConnectorRegistry>,
}

impl Drop for PluginHostManager {
    fn drop(&mut self) {
        if let Some(task) = self.health_task.take() {
            task.abort();
        }
        // Best-effort graceful shutdown: kill child processes and wait.
        // async shutdown() was not called, so we cannot send Shutdown IPC
        // messages, but each SupervisedPlugin::drop handles process cleanup.
        // The caller should prefer calling shutdown().await before drop.
        if self.shutdown_on_drop {
            tracing::warn!(
                component = "PluginHostManager",
                "PluginHostManager dropped without explicit shutdown(); \
                 child processes will be killed without graceful IPC shutdown"
            );
        }
    }
}

impl PluginHostManager {
    /// Discovers and starts configured plugin binaries, performing handshakes
    /// and capability routing. Also connects to configured MCP servers.
    ///
    /// Only plugins listed in `plugins.list` with `enable: true` are started
    /// (opt-in). Binaries found on disk that are not listed emit a warning.
    ///
    /// `db_tokens` maps plugin names to pre-shared DB IPC auth tokens; tool
    /// plugins that need database access receive their token via the sandbox
    /// config during the handshake.
    ///
    /// Respects the `plugins` config section: when `plugins.enabled` is
    /// `false`, no plugins are started. Individual plugins can be disabled
    /// via `plugins.list.<name>.enable = false`.
    pub async fn start(
        config: &EneConfig,
        db_tokens: HashMap<String, String>,
    ) -> Result<Self, PluginHostError> {
        let plugin_config = config
            .get_section::<crate::config::PluginConfig>()
            .unwrap_or_default();

        if !plugin_config.enabled {
            tracing::info!(
                component = "PluginHostManager",
                "Plugin system disabled by configuration"
            );
            return Ok(Self {
                supervised: Vec::new(),
                connections: Vec::new(),
                tool_registries: Vec::new(),
                llm_factories: HashMap::new(),
                health_task: None,
                health_rx: None,
                shutdown_on_drop: true,
                connector_registry: Arc::new(ConnectorRegistry::new()),
            });
        }

        let (health_tx, health_rx) = mpsc::unbounded_channel::<PluginHealthEvent>();

        let mut supervised: Vec<Arc<Mutex<SupervisedPlugin>>> = Vec::new();
        let mut connections: Vec<Arc<Mutex<IpcPluginConnection>>> = Vec::new();
        let mut tool_registries: Vec<Arc<dyn ToolRegistry>> = Vec::new();
        let mut llm_factories: HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>> =
            HashMap::new();

        std::fs::create_dir_all(ene_config::plugin_socket_dir()).map_err(|e| {
            PluginHostError::ExecutionFailed {
                message: format!("Failed to create plugin socket dir: {e}"),
            }
        })?;

        // Warn about plugin binaries on disk that are NOT listed in config.
        for name in discover_plugins() {
            if !plugin_config.list.contains_key(&name) {
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin = %name,
                    "Plugin binary found on disk but not listed in plugins.list; \
                     add 'plugins.list.{name}.enable = true' to settings.json to activate it"
                );
            }
        }

        // TOFU checksums to persist after startup completes.
        let mut checksums_to_record: HashMap<String, String> = HashMap::new();

        for (name, entry) in &plugin_config.list {
            if !entry.enable {
                tracing::info!(
                    component = "PluginHostManager",
                    plugin = %name,
                    "Plugin disabled by configuration; skipping"
                );
                continue;
            }

            // Reject names that could escape the plugin directory via
            // path traversal before any filesystem access.
            if !is_valid_plugin_name(name) {
                tracing::error!(
                    component = "PluginHostManager",
                    plugin = %name,
                    "Invalid plugin name (must match [a-zA-Z0-9_-] with no path \
                     separators or '..'); skipping"
                );
                continue;
            }

            let entry_config = Some(entry.config.clone())
                .filter(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()));

            match Self::start_plugin(
                name,
                entry_config,
                entry.checksum.clone(),
                entry.env_passthrough.clone(),
                db_tokens.get(name).cloned(),
            )
            .await
            {
                Ok((plugin, conn, tofu_checksum)) => {
                    if let Some(checksum) = tofu_checksum {
                        checksums_to_record.insert(name.clone(), checksum);
                    }

                    let caps = conn.lock().await.capabilities().clone();

                    // Route tool capabilities — fetch actual specs via ListTools.
                    if caps.tools > 0 {
                        // Retry once on failure; if it still fails, skip
                        // registering the tool registry (don't silently
                        // register empty tools). The plugin process itself is
                        // still supervised so any LLM providers it offers keep
                        // working.
                        let tools = match conn.lock().await.list_tools().await {
                            Ok(tools) => Some(tools),
                            Err(e) => {
                                tracing::warn!(
                                    component = "PluginHostManager",
                                    plugin = %name,
                                    error = %e,
                                    "Failed to list tools for plugin; retrying once"
                                );
                                match conn.lock().await.list_tools().await {
                                    Ok(tools) => Some(tools),
                                    Err(e) => {
                                        tracing::error!(
                                            component = "PluginHostManager",
                                            plugin = %name,
                                            error = %e,
                                            "Failed to list tools for plugin after retry; \
                                             skipping tool registry registration"
                                        );
                                        None
                                    }
                                }
                            }
                        };
                        if let Some(tools) = tools {
                            let registry = PluginToolRegistry {
                                plugin_name: name.clone(),
                                conn: Arc::clone(&conn),
                                supervised: Arc::clone(&plugin),
                                tools,
                                breaker: parking_lot::Mutex::new(CircuitBreaker::default()),
                                health_tx: Some(health_tx.clone()),
                            };
                            tool_registries.push(Arc::new(registry));
                        }
                    }

                    // Route LLM provider capabilities.
                    for spec in &caps.llm_providers {
                        if llm_factories.contains_key(&spec.kind) {
                            tracing::warn!(
                                component = "PluginHostManager",
                                plugin = %name,
                                kind = %spec.kind,
                                "Duplicate LLM provider kind; skipping"
                            );
                            continue;
                        }
                        let factory = IpcLlmProviderFactory::new(
                            spec.kind.clone(),
                            Arc::clone(&conn),
                            name.clone(),
                            is_builtin_plugin(name),
                            spec.concurrency,
                        );
                        llm_factories.insert(
                            spec.kind.clone(),
                            Arc::new(factory) as Arc<dyn ene_ai::LlmProviderFactory>,
                        );
                    }

                    supervised.push(plugin);
                    connections.push(conn);
                }
                Err(e) => {
                    tracing::error!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Failed to start plugin"
                    );
                }
            }
        }

        // Batch-persist TOFU checksums recorded during this startup.
        //
        // NOTE: `updated_config` is derived from the in-memory `config`
        // parameter captured at process startup. `ene_config::update_section`
        // re-reads the file from disk before overwriting the `plugins`
        // section, so concurrent edits to *other* sections are preserved.
        // However, if another process modified the `plugins` section between
        // our startup read and this write, those changes are lost. This is
        // acceptable because only a single host process writes plugin config
        // at startup; a dedicated atomic writer may replace this in future.
        if !checksums_to_record.is_empty() {
            let mut updated_config = config
                .get_section::<crate::config::PluginConfig>()
                .unwrap_or_default();
            for (name, checksum) in &checksums_to_record {
                if let Some(e) = updated_config.list.get_mut(name) {
                    e.checksum = Some(checksum.clone());
                }
            }
            if let Err(e) = ene_config::update_section(&updated_config) {
                tracing::warn!(
                    component = "PluginHostManager",
                    error = %e,
                    "Failed to persist plugin checksums to configuration"
                );
            }
        }

        // Connect to configured MCP servers via McpConnector (WS7: wire into lifecycle).
        let connector_registry = Arc::new(ConnectorRegistry::new());
        if !plugin_config.mcp_servers.is_empty() {
            for server in &plugin_config.mcp_servers {
                if !server.enabled {
                    continue;
                }

                let connector = McpConnector::new(server.clone());
                let registry = connector.registry();

                // Register the connector for lifecycle management
                if let Err(err) = connector_registry.register(Box::new(connector)) {
                    tracing::warn!(
                        component = "PluginHostManager",
                        server = %server.name,
                        error = %err,
                        "Failed to register MCP connector"
                    );
                    continue;
                }

                // Connect the MCP server
                match &server.transport {
                    McpTransport::Stdio { command, args } => {
                        let args_ref: Vec<&str> =
                            args.iter().map(std::string::String::as_str).collect();
                        if let Err(err) = registry
                            .connect_stdio(
                                &server.name,
                                command,
                                &args_ref,
                                &server.env_passthrough,
                            )
                            .await
                        {
                            tracing::warn!(
                                component = "PluginHostManager",
                                server = %server.name,
                                error = %err,
                                "MCP server failed to connect"
                            );
                        }
                    }
                    McpTransport::Http { url, auth_header } => {
                        tracing::warn!(
                            component = "PluginHostManager",
                            server = %server.name,
                            url = %url,
                            "Attempting MCP server connection via HTTP (streamable HTTP transport); \
                             this path may fail if the server is unreachable or incompatible"
                        );
                        if let Err(err) = registry
                            .connect_http(&server.name, url, auth_header.as_deref())
                            .await
                        {
                            tracing::warn!(
                                component = "PluginHostManager",
                                server = %server.name,
                                url = %url,
                                error = %err,
                                "MCP HTTP connection failed"
                            );
                        }
                    }
                }

                // Add the registry to tool registries
                tool_registries.push(registry);
            }
        }

        // Spawn the periodic health probe loop (disabled when the interval is 0).
        let health_interval = Duration::from_millis(plugin_config.health_interval_ms);
        let health_task = if supervised.is_empty() || health_interval.is_zero() {
            if health_interval.is_zero() && !supervised.is_empty() {
                tracing::info!(
                    component = "PluginHostManager",
                    "Health probes disabled by configuration (health_interval_ms = 0)"
                );
            }
            None
        } else {
            let probes: Vec<Arc<Mutex<SupervisedPlugin>>> =
                supervised.iter().map(Arc::clone).collect();
            let conns: Vec<Arc<Mutex<IpcPluginConnection>>> =
                connections.iter().map(Arc::clone).collect();
            let tx = health_tx.clone();
            Some(tokio::spawn(async move {
                health_probe_loop(health_interval, probes, conns, tx).await;
            }))
        };

        Ok(Self {
            supervised,
            connections,
            tool_registries,
            llm_factories,
            health_task,
            health_rx: Some(health_rx),
            shutdown_on_drop: true,
            connector_registry,
        })
    }

    /// Returns the tool registries contributed by plugins and MCP servers.
    pub fn tool_registries(&self) -> &[Arc<dyn ToolRegistry>] {
        &self.tool_registries
    }

    /// Returns the LLM provider factories contributed by plugins, keyed by
    /// provider kind.
    pub fn llm_factories(&self) -> &HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>> {
        &self.llm_factories
    }

    /// Returns the connector registry managing MCP server connectors (WS7).
    pub fn connector_registry(&self) -> &Arc<ConnectorRegistry> {
        &self.connector_registry
    }

    /// Takes ownership of the health-event receiver.
    ///
    /// The runtime calls this once after startup to bridge plugin health
    /// events into its diagnostics channel. Returns `None` on subsequent
    /// calls.
    pub fn take_health_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<PluginHealthEvent>> {
        self.health_rx.take()
    }

    /// Controls whether Drop attempts best-effort shutdown.
    /// Set to false if the caller will handle shutdown explicitly.
    pub fn set_shutdown_on_drop(&mut self, value: bool) {
        self.shutdown_on_drop = value;
    }

    /// Sends a graceful `Shutdown` to all plugins and kills the processes.
    pub async fn shutdown(&mut self) {
        // Abort the health probe loop first so it cannot race with shutdown
        // (e.g. restart a plugin we are about to kill).
        if let Some(task) = self.health_task.take() {
            task.abort();
        }
        for conn in &self.connections {
            let mut c = conn.lock().await;
            c.shutdown().await;
        }
        for plugin in &self.supervised {
            let mut p = plugin.lock().await;
            drop(p.child.kill());
            drop(p.child.wait());
            ene_plugin_proto::cleanup_path(&p.socket_path);
        }
    }

    async fn start_plugin(
        name: &str,
        plugin_config: Option<serde_json::Value>,
        expected_checksum: Option<String>,
        env_passthrough: Vec<String>,
        db_token: Option<String>,
    ) -> Result<
        (
            Arc<Mutex<SupervisedPlugin>>,
            Arc<Mutex<IpcPluginConnection>>,
            Option<String>,
        ),
        PluginHostError,
    > {
        let binary_path = find_plugin_binary(name).ok_or_else(|| PluginHostError::SpawnFailed {
            name: name.to_string(),
            reason: "binary not found".to_string(),
        })?;

        // Verify binary checksum. When no checksum is configured, compute
        // and return it for trust-on-first-use recording.
        let tofu_checksum =
            verify_and_record_checksum(name, &binary_path, expected_checksum.as_deref())?;

        let socket_path: PathBuf = {
            #[cfg(unix)]
            {
                let p = ene_config::plugin_socket_dir().join(format!("ene-plugin-{name}.sock"));
                if p.exists() {
                    drop(std::fs::remove_file(&p));
                }
                p
            }
            #[cfg(windows)]
            {
                PathBuf::from(format!(r"\\.\pipe\ene-plugin-{}", name))
            }
        };

        let mut sandbox = ene_plugin_proto::SandboxConfigData::default();
        if let Some(token) = &db_token {
            sandbox.db_auth_token = Some(token.clone());
            #[cfg(unix)]
            {
                let db_socket =
                    ene_config::paths::tool_socket_dir().join(format!("ene-db-{name}.sock"));
                sandbox.db_socket = Some(db_socket.to_string_lossy().to_string());
            }
        }

        let child = build_plugin_command(&binary_path, &socket_path, &env_passthrough)
            .spawn()
            .map_err(|e| PluginHostError::SpawnFailed {
                name: name.to_string(),
                reason: e.to_string(),
            })?;

        let conn =
            IpcPluginConnection::connect(&socket_path, sandbox.clone(), plugin_config.clone())
                .await?;

        // The pinned checksum for restart-time verification: the configured
        // checksum when present, otherwise the trust-on-first-use checksum
        // just recorded. Either way the binary that was verified at startup
        // is pinned and re-verified on every restart (#429).
        let pinned_checksum = match expected_checksum {
            Some(configured) => PinnedChecksum::Configured(configured),
            None => match tofu_checksum.clone() {
                Some(tofu) => PinnedChecksum::Tofu(tofu),
                // Unreachable: `verify_and_record_checksum` always records a
                // TOFU checksum when none is configured. Recompute defensively
                // rather than panic so this stays panic-free.
                None => PinnedChecksum::Tofu(compute_binary_checksum(name, &binary_path)?),
            },
        };

        let plugin = SupervisedPlugin {
            name: name.to_string(),
            child,
            socket_path: socket_path.clone(),
            binary_path: binary_path.clone(),
            pinned_checksum,
            sandbox,
            plugin_config,
            env_passthrough,
            restart_count: 0,
            disabled: false,
        };

        Ok((
            Arc::new(Mutex::new(plugin)),
            Arc::new(Mutex::new(conn)),
            tofu_checksum,
        ))
    }
}

/// Discovers plugin binary names by scanning the builtin and user plugin
/// directories for executables following the `ene-plugin-{name}` naming
/// convention.
///
/// Only files whose name starts with `ene-plugin-` and that are executable
/// are treated as plugins. This strictness is required because, in debug
/// builds, [`builtin_plugins_dir`](ene_config::builtin_plugins_dir) resolves
/// to the directory of the running executable — for test binaries that is
/// `target/debug/deps`, which is full of non-plugin build artifacts
/// (`.rlib`, `.rmeta`, `.d`, object files) **and other test binaries**. A
/// permissive match would cause the host to spawn every one of those
/// (including test binaries that themselves start a plugin host), producing
/// an unbounded recursive process explosion.
fn discover_plugins() -> Vec<String> {
    let mut names = Vec::new();
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };

    for dir in [
        ene_config::builtin_plugins_dir(),
        ene_config::user_plugins_dir(),
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_executable(&path) {
                continue;
            }
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Strip the exe suffix for matching.
            let stem = file_name
                .strip_suffix(exe_suffix)
                .unwrap_or(&file_name)
                .to_string();

            // Only the `ene-plugin-{name}` convention is accepted; the bare
            // `{name}` fallback is intentionally omitted (see fn docs).
            let Some(plugin_name) = stem.strip_prefix("ene-plugin-") else {
                continue;
            };
            let plugin_name = plugin_name.to_string();

            if !plugin_name.is_empty() && !names.contains(&plugin_name) {
                names.push(plugin_name);
            }
        }
    }

    names
}

/// Returns `true` when `path` has an executable permission bit set.
///
/// On Unix this checks the mode bits; on non-Unix targets every existing
/// file is considered executable (the `.exe` suffix already gates matching
/// there).
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// Validates that a plugin name is safe for use in filesystem paths.
///
/// Rejects names containing path separators, parent-directory traversal
/// (`..`), or characters outside the safe set `[a-zA-Z0-9_-]`. This
/// prevents config keys like `x/../../etc/evil` from escaping the plugin
/// directory.
fn is_valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Returns `true` when the plugin binary resolves from the builtin plugins
/// directory. Used by the API key trust gate: only builtin or explicitly
/// configured plugins receive resolved credentials.
fn is_builtin_plugin(name: &str) -> bool {
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    let dir = ene_config::builtin_plugins_dir();
    dir.join(format!("ene-plugin-{name}{exe_suffix}")).is_file()
        || dir.join(format!("{name}{exe_suffix}")).is_file()
}

/// Finds the binary path for a plugin by name, searching builtin and user
/// directories with both `ene-plugin-{name}` and `{name}` naming conventions.
///
/// Returns `None` if the name fails [`is_valid_plugin_name`] validation or
/// no matching binary exists.
fn find_plugin_binary(name: &str) -> Option<PathBuf> {
    if !is_valid_plugin_name(name) {
        return None;
    }

    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };

    let builtin_dir = ene_config::builtin_plugins_dir();
    let user_dir = ene_config::user_plugins_dir();

    let candidates = [
        builtin_dir.join(format!("ene-plugin-{name}{exe_suffix}")),
        builtin_dir.join(format!("{name}{exe_suffix}")),
        user_dir.join(format!("ene-plugin-{name}{exe_suffix}")),
        user_dir.join(format!("{name}{exe_suffix}")),
    ];

    candidates.into_iter().find(|c| c.is_file())
}

/// Background loop that periodically pings every supervised plugin and
/// restarts any that are dead or unresponsive, emitting health events.
#[expect(
    clippy::infinite_loop,
    reason = "background liveness probe runs for the lifetime of the plugin host"
)]
async fn health_probe_loop(
    interval: Duration,
    plugins: Vec<Arc<Mutex<SupervisedPlugin>>>,
    connections: Vec<Arc<Mutex<IpcPluginConnection>>>,
    health_tx: mpsc::UnboundedSender<PluginHealthEvent>,
) {
    let mut ticker = tokio::time::interval(interval);
    // Skip the immediate first tick.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        for (plugin, conn) in plugins.iter().zip(connections.iter()) {
            // Permanently disabled plugins (restart budget exhausted or a
            // restart-time checksum mismatch) are left stopped; skip them so
            // the probe does not keep restarting or re-emitting `Disabled`.
            if plugin.lock().await.disabled {
                continue;
            }

            let ping_ok = {
                let mut c = conn.lock().await;
                c.ping().await.is_ok()
            };
            let alive = {
                let mut p = plugin.lock().await;
                p.is_alive()
            };

            if alive && ping_ok {
                continue;
            }

            let reason = if alive { "unresponsive" } else { "dead" };
            let name = {
                let p = plugin.lock().await;
                p.name.clone()
            };

            drop(health_tx.send(PluginHealthEvent::Unhealthy {
                plugin: name.clone(),
                reason: reason.to_string(),
            }));

            tracing::warn!(
                component = "PluginHostManager",
                plugin = %name,
                reason = reason,
                "Health probe: plugin unhealthy; restarting"
            );

            // Restart the plugin process and reconnect.
            let mut p = plugin.lock().await;
            if p.restart_count >= MAX_RESTARTS {
                tracing::error!(
                    component = "PluginHostManager",
                    plugin = %name,
                    "Plugin exceeded max restarts; disabled"
                );
                p.disabled = true;
                drop(health_tx.send(PluginHealthEvent::Disabled {
                    plugin: name,
                    reason: crate::health::DisabledReason::RestartBudgetExhausted,
                }));
                continue;
            }

            let attempt = p.restart_count.saturating_add(1);
            drop(health_tx.send(PluginHealthEvent::Restarting {
                plugin: name.clone(),
                attempt,
            }));

            let delay = p.delay_for_restart();
            drop(p);
            tokio::time::sleep(delay).await;

            let mut p = plugin.lock().await;
            if let Err(e) = p.restart() {
                // A checksum mismatch means the on-disk binary changed since
                // it was last verified. Do NOT silently retry: disable the
                // plugin and tell the user via a diagnostic (#429).
                if matches!(e, PluginHostError::ChecksumMismatch { .. }) {
                    tracing::error!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Plugin binary checksum mismatch on restart; \
                         binary changed since last verification; disabling plugin"
                    );
                    p.disabled = true;
                    drop(health_tx.send(PluginHealthEvent::Disabled {
                        plugin: name,
                        reason: crate::health::DisabledReason::ChecksumMismatch,
                    }));
                } else {
                    tracing::error!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Failed to restart plugin"
                    );
                }
                continue;
            }

            let socket_path = p.socket_path.clone();
            let sandbox = p.sandbox.clone();
            let plugin_config = p.plugin_config.clone();
            drop(p);

            match IpcPluginConnection::connect(&socket_path, sandbox, plugin_config).await {
                Ok(new_conn) => {
                    let mut c = conn.lock().await;
                    *c = new_conn;
                    drop(health_tx.send(PluginHealthEvent::Restarted {
                        plugin: name.clone(),
                    }));
                    drop(health_tx.send(PluginHealthEvent::Recovered {
                        plugin: name.clone(),
                    }));
                    tracing::info!(
                        component = "PluginHostManager",
                        plugin = %name,
                        "Plugin restarted and reconnected"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Failed to reconnect after restart"
                    );
                }
            }
        }
    }
}

/// Computes the SHA-256 checksum of a plugin binary, hex-encoded.
///
/// The file is hashed in fixed-size 64KiB chunks read directly from the
/// [`std::fs::File`] so the entire binary is never held in memory at once —
/// llama-linked plugin binaries can exceed 100MB, and this runs on every
/// restart (#429). A `BufReader` is deliberately not used: its 8KiB internal
/// buffer is bypassed when reading into a buffer at least that large, so it
/// would add a layer without reducing syscalls; a 64KiB buffer keeps the
/// syscall count low for large binaries.
fn compute_binary_checksum(
    plugin_name: &str,
    path: &std::path::Path,
) -> Result<String, PluginHostError> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| PluginHostError::SpawnFailed {
        name: plugin_name.to_string(),
        reason: format!("cannot read binary for checksum verification: {e}"),
    })?;

    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| PluginHostError::SpawnFailed {
                name: plugin_name.to_string(),
                reason: format!("cannot read binary for checksum verification: {e}"),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verifies a plugin binary against an expected checksum.
///
/// Computes the streaming SHA-256 hash of the binary and returns
/// [`PluginHostError::ChecksumMismatch`] if it doesn't match `expected`.
/// There is no "no checksum" path: callers always have a pinned checksum
/// (configured or trust-on-first-use), so verification never fails open (#429).
fn verify_plugin_checksum(
    plugin_name: &str,
    path: &std::path::Path,
    expected: &str,
) -> Result<(), PluginHostError> {
    let actual = compute_binary_checksum(plugin_name, path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(PluginHostError::ChecksumMismatch {
            name: plugin_name.to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    tracing::info!(
        component = "PluginHostManager",
        plugin = %plugin_name,
        "Plugin binary checksum verified"
    );
    Ok(())
}

/// Verifies the plugin binary checksum at startup and computes a TOFU
/// checksum when none is configured yet.
///
/// When `expected` is `Some`, verifies via [`verify_plugin_checksum`] and
/// returns `Ok(None)` (nothing new to record). When `expected` is `None`,
/// computes the checksum and returns it as `Ok(Some(checksum))` for the
/// caller to persist (trust-on-first-use).
fn verify_and_record_checksum(
    plugin_name: &str,
    path: &std::path::Path,
    expected: Option<&str>,
) -> Result<Option<String>, PluginHostError> {
    let Some(expected) = expected else {
        // Trust-on-first-use: record the checksum for future verification.
        let actual = compute_binary_checksum(plugin_name, path)?;
        tracing::info!(
            component = "PluginHostManager",
            plugin = %plugin_name,
            checksum = %actual,
            "Recording plugin binary checksum (trust-on-first-use)"
        );
        return Ok(Some(actual));
    };

    verify_plugin_checksum(plugin_name, path, expected)?;
    Ok(None)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise failure messages"
)]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "test-only set_var/remove_var; serialized via ENV_MUTEX"
)]
mod tests {
    use super::*;

    /// Serializes tests that mutate the process environment (`set_var` /
    /// `remove_var` are unsound when concurrent threads read the env).
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn delay_for_restart_grows_then_caps() {
        assert_eq!(delay_for_restart(0), Duration::from_millis(500));
        assert_eq!(delay_for_restart(1), Duration::from_secs(1));
        assert_eq!(delay_for_restart(2), Duration::from_secs(2));
        assert_eq!(delay_for_restart(3), Duration::from_secs(4));
        assert_eq!(delay_for_restart(4), Duration::from_secs(8));
        assert_eq!(delay_for_restart(30), Duration::from_millis(MAX_DELAY_MS));
    }

    #[test]
    fn discover_plugins_empty_dirs() {
        // With no plugin directories present, discovery returns empty.
        let plugins = discover_plugins();
        // We can't assert emptiness in CI (there might be plugins), but
        // the function must not panic.
        let _ = plugins;
    }

    #[test]
    fn find_plugin_binary_nonexistent() {
        assert!(find_plugin_binary("nonexistent-plugin-xyz").is_none());
    }

    #[test]
    fn find_plugin_binary_rejects_path_traversal() {
        assert!(find_plugin_binary("../etc/passwd").is_none());
        assert!(find_plugin_binary("foo/bar").is_none());
        assert!(find_plugin_binary("..").is_none());
        assert!(find_plugin_binary("").is_none());
    }

    #[test]
    fn is_valid_plugin_name_accepts_safe_names() {
        assert!(is_valid_plugin_name("fs"));
        assert!(is_valid_plugin_name("ene-plugin-web"));
        assert!(is_valid_plugin_name("my_plugin_2"));
    }

    #[test]
    fn is_valid_plugin_name_rejects_unsafe_names() {
        assert!(!is_valid_plugin_name(""));
        assert!(!is_valid_plugin_name(".."));
        assert!(!is_valid_plugin_name("x/../../etc/evil"));
        assert!(!is_valid_plugin_name("foo\\bar"));
        assert!(!is_valid_plugin_name("has space"));
        assert!(!is_valid_plugin_name("semi;colon"));
    }

    #[test]
    fn checksum_mismatch_blocks_verification() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("ene-checksum-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin_path = dir.join("ene-plugin-fake");
        {
            let mut f = std::fs::File::create(&bin_path).unwrap();
            f.write_all(b"fake plugin binary contents").unwrap();
        }

        // Compute the real checksum of the file.
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"fake plugin binary contents");
        let real = hex::encode(hasher.finalize());

        // The streaming helper must produce the same digest as the in-memory
        // reference hash (it reads in chunks, never the whole file).
        assert_eq!(compute_binary_checksum("fake", &bin_path).unwrap(), real);

        // Matching checksum passes, no TOFU recording.
        assert!(verify_and_record_checksum("fake", &bin_path, Some(&real)).is_ok());
        assert_eq!(
            verify_and_record_checksum("fake", &bin_path, Some(&real)).unwrap(),
            None
        );

        // Case-insensitive comparison: uppercase hex also matches.
        let upper = real.to_ascii_uppercase();
        assert!(verify_and_record_checksum("fake", &bin_path, Some(&upper)).is_ok());

        // Mismatched checksum is rejected with ChecksumMismatch.
        let result = verify_and_record_checksum("fake", &bin_path, Some("deadbeef"));
        assert!(matches!(
            result,
            Err(PluginHostError::ChecksumMismatch { .. })
        ));

        // No checksum configured → trust-on-first-use, returns computed hash.
        let tofu = verify_and_record_checksum("fake", &bin_path, None).unwrap();
        assert_eq!(tofu, Some(real));

        drop(std::fs::remove_dir_all(&dir));
    }

    /// Builds a [`SupervisedPlugin`] for restart-verification tests.
    ///
    /// `binary_path` points at a file in a fresh temp dir and `child` is a
    /// trivial real process (`sh -c "exit 0"`, PATH-resolved so it works on
    /// NixOS where `/bin/true` does not exist) so `restart()` has something
    /// to kill/reap. Returns the plugin.
    #[cfg(unix)]
    fn supervised_plugin_with(
        binary_path: PathBuf,
        pinned_checksum: PinnedChecksum,
    ) -> SupervisedPlugin {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let socket_path = binary_path.parent().unwrap().join("test.sock");

        SupervisedPlugin {
            name: "fake".to_string(),
            child,
            socket_path,
            binary_path,
            pinned_checksum,
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
            env_passthrough: Vec::new(),
            restart_count: 0,
            disabled: false,
        }
    }

    /// Writes a minimal executable shell script (`#!/usr/bin/env sh\nexit 0`)
    /// to `path` with `0o755` permissions. The shebang resolves through
    /// `/usr/bin/env`, which exists on NixOS (unlike `/bin/true`), so
    /// `restart()` can genuinely re-spawn the file.
    #[cfg(unix)]
    fn write_dummy_script(path: &std::path::Path) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"#!/usr/bin/env sh\nexit 0\n").unwrap();
        drop(f);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn temp_plugin_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ene-restart-checksum-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    #[cfg(unix)]
    fn restart_verifies_checksum_when_matching() {
        // `binary_path` is a genuine executable script so the matching-checksum
        // path can actually re-spawn it. The shebang uses `/usr/bin/env sh`,
        // which exists on NixOS (unlike `/bin/true`).
        let dir = temp_plugin_dir("match");
        let binary_path = dir.join("ene-plugin-fake");
        write_dummy_script(&binary_path);

        let real = compute_binary_checksum("fake", &binary_path).unwrap();
        let mut plugin = supervised_plugin_with(binary_path.clone(), PinnedChecksum::Tofu(real));

        // Matching checksum: verification passes and the plugin is respawned.
        assert!(plugin.restart().is_ok());
        assert_eq!(plugin.restart_count, 1);

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    #[cfg(unix)]
    fn restart_rejects_tampered_binary() {
        use std::io::Write;

        // Here `binary_path` is a plain data file (never executed), so it can
        // be freely rewritten to simulate a binary swap; `restart()` aborts on
        // the mismatch before ever trying to spawn it.
        let dir = temp_plugin_dir("tamper");
        let binary_path = dir.join("ene-plugin-fake");
        std::fs::write(&binary_path, b"original plugin binary contents").unwrap();

        // Pin the checksum of the original binary.
        let real = compute_binary_checksum("fake", &binary_path).unwrap();
        let mut plugin = supervised_plugin_with(binary_path.clone(), PinnedChecksum::Tofu(real));

        // Tamper with the binary on disk after the checksum was recorded.
        {
            let mut f = std::fs::File::create(&binary_path).unwrap();
            f.write_all(b"tampered binary contents that differ from the original")
                .unwrap();
        }

        // Restart must abort with a checksum mismatch...
        let result = plugin.restart();
        assert!(matches!(
            result,
            Err(PluginHostError::ChecksumMismatch { .. })
        ));
        // ...and the attempt consumed one unit of restart budget. Verification
        // happens before spawning, so the tampered binary was never exec'd; the
        // dead/hung child is reaped on the mismatch path (no zombie left).
        assert_eq!(plugin.restart_count, 1);

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn build_plugin_command_clears_environment() {
        use std::ffi::OsStr;

        let binary = std::path::Path::new("/usr/bin/env");
        let socket = std::path::Path::new("/tmp/test.sock");

        let cmd = build_plugin_command(binary, socket, &[]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();

        // Must contain the IPC socket.
        assert!(envs.contains_key(OsStr::new("ENE_PLUGIN_SOCKET")));
        // Must contain whitelisted vars (when present in host env).
        assert!(envs.contains_key(OsStr::new("PATH")));
        // Must NOT contain arbitrary secrets.
        assert!(!envs.contains_key(OsStr::new("ANTHROPIC_API_KEY")));
        assert!(!envs.contains_key(OsStr::new("AWS_SECRET_ACCESS_KEY")));
    }

    #[test]
    fn build_plugin_command_respects_passthrough() {
        use std::ffi::OsStr;

        let _guard = ENV_MUTEX.lock().unwrap();

        let binary = std::path::Path::new("/usr/bin/env");
        let socket = std::path::Path::new("/tmp/test.sock");

        // Set a variable in the current process to verify passthrough.
        unsafe { std::env::set_var("ENE_TEST_PASSTHROUGH_VAR", "hello") };
        let cmd = build_plugin_command(binary, socket, &["ENE_TEST_PASSTHROUGH_VAR".to_string()]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        assert!(envs.contains_key(OsStr::new("ENE_TEST_PASSTHROUGH_VAR")));
        unsafe { std::env::remove_var("ENE_TEST_PASSTHROUGH_VAR") };

        // Without passthrough, the variable must not leak.
        unsafe { std::env::set_var("ENE_TEST_LEAKED_VAR", "secret") };
        let cmd = build_plugin_command(binary, socket, &[]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        assert!(!envs.contains_key(OsStr::new("ENE_TEST_LEAKED_VAR")));
        unsafe { std::env::remove_var("ENE_TEST_LEAKED_VAR") };
    }

    #[test]
    fn build_plugin_command_blocks_denylisted_passthrough() {
        use std::ffi::OsStr;

        let _guard = ENV_MUTEX.lock().unwrap();

        let binary = std::path::Path::new("/usr/bin/env");
        let socket = std::path::Path::new("/tmp/test.sock");

        // LD_PRELOAD must not be forwarded even if explicitly requested.
        unsafe { std::env::set_var("LD_PRELOAD", "/tmp/evil.so") };
        let cmd = build_plugin_command(binary, socket, &["LD_PRELOAD".to_string()]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        assert!(!envs.contains_key(OsStr::new("LD_PRELOAD")));
        unsafe { std::env::remove_var("LD_PRELOAD") };

        // ENE_PLUGIN_SOCKET must not be overridable via passthrough.
        let cmd = build_plugin_command(binary, socket, &["ENE_PLUGIN_SOCKET".to_string()]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        // The socket is set by build_plugin_command itself, not by passthrough.
        // Verify it points to the expected socket path.
        assert_eq!(
            envs.get(OsStr::new("ENE_PLUGIN_SOCKET")),
            Some(&Some(std::ffi::OsStr::new("/tmp/test.sock")))
        );
    }
}
