//! Plugin host manager: process supervision, capability routing, and lifecycle.
//!
//! [`PluginHostManager`] starts only plugins explicitly listed in
//! `plugins.list` with `enable: true` (opt-in discovery). Each plugin is
//! spawned with a hardened environment (`env_clear()` + whitelist), performs
//! the v3 handshake, and routes advertised capabilities (tools, LLM
//! providers) into the appropriate host registries. It also connects to
//! configured MCP servers and exposes their tools alongside plugin-provided
//! tools.

use std::collections::{HashMap, VecDeque};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ene_config::EneConfig;
use tokio::sync::{Mutex, mpsc};

use sha2::Digest;

use crate::capability_registry::{
    CapabilityDeclaration, CapabilityRegistry, evaluate_capability_gate,
};
use crate::circuit_breaker::CircuitBreaker;
use crate::credential_registry::CredentialRegistry;
use crate::embedding::IpcEmbeddingProviderFactory;
use crate::error::PluginHostError;
use crate::factory::IpcLlmProviderFactory;
use crate::health::PluginHealthEvent;
use crate::ipc_plugin::IpcPluginConnection;
use crate::mcp_config::McpTransport;
use crate::mcp_registry::{McpToolRegistry, redacted_endpoint};
use crate::tool_registry::{DeferredCallResult, ToolRegistry};
use crate::tts_factory::IpcTtsProviderFactory;
use ene_connector::declaration::{CredentialDeclaration, ScopeDecision};
use ene_connector::identity::CredentialId;

/// Maximum restarts allowed inside [`RESTART_WINDOW`] before a plugin is disabled.
const MAX_RESTARTS: usize = 5;
/// Rolling window over which [`MAX_RESTARTS`] is counted.
const RESTART_WINDOW: Duration = Duration::from_mins(5);
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
/// verifies its binary on restart (no fail-open path).
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
    /// restart. There is no "no checksum" state, so no fail-open path.
    pinned_checksum: PinnedChecksum,
    /// Environment variable names to copy from the host on restart.
    env_passthrough: Vec<String>,
    /// Timestamps of restarts inside the rolling [`RESTART_WINDOW`].
    ///
    /// Entries older than the window are dropped on each budget check, so a
    /// plugin that crashes occasionally never exhausts the budget, while a
    /// crash loop within the window is disabled. Healthy probes and tool
    /// calls do not clear this window — only time does.
    restart_times: VecDeque<Instant>,
    /// Set once the plugin is permanently disabled (restart budget exhausted
    /// or a restart-time checksum mismatch). The per-plugin supervisor task
    /// skips disabled plugins so it does not keep restarting or re-emitting
    /// `PluginHealthEvent::Disabled` for them.
    ///
    /// Terminal for the life of the host: nothing clears this flag, and the
    /// supervisor exits its loop on observing it. The rolling
    /// [`RESTART_WINDOW`] therefore governs only whether the limit is *reached*
    /// — draining the window afterwards cannot bring a disabled plugin back.
    /// Recovery is a host restart (or a `plugins.list` reconfiguration, which
    /// builds a fresh [`SupervisedPlugin`]).
    disabled: bool,
}

impl SupervisedPlugin {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Drops restart timestamps older than [`RESTART_WINDOW`] and returns how
    /// many remain. Sole source of the restart-budget length.
    fn prune_restart_window(&mut self, now: Instant) -> usize {
        while self
            .restart_times
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > RESTART_WINDOW)
        {
            self.restart_times.pop_front();
        }
        self.restart_times.len()
    }

    /// Restarts counted inside the current rolling window (after pruning).
    fn recent_restart_count(&mut self) -> usize {
        self.prune_restart_window(Instant::now())
    }

    /// Attempts a restart, recording it against the rolling window budget.
    ///
    /// Returns [`PluginHostError::ExecutionFailed`] when [`MAX_RESTARTS`]
    /// restarts have already occurred inside [`RESTART_WINDOW`]. This is the
    /// only place the budget limit is enforced; the supervisor treats any
    /// `Err` (budget or checksum) as a permanent disable.
    fn restart(&mut self) -> Result<(), PluginHostError> {
        let now = Instant::now();
        let recent = self.prune_restart_window(now);
        if recent >= MAX_RESTARTS {
            return Err(PluginHostError::ExecutionFailed {
                message: format!(
                    "Plugin '{}' exceeded max restarts ({MAX_RESTARTS} in {RESTART_WINDOW:?})",
                    self.name
                ),
            });
        }
        self.restart_times.push_back(now);
        let attempt = self.restart_times.len();

        // Verify the binary checksum BEFORE killing the running process or
        // spawning a replacement. At this point the process is already dead
        // or unresponsive (that's why the health probe triggered a restart).
        // The goal of verifying BEFORE kill/spawn is to avoid exec'ing a
        // tampered binary — not to preserve a "good" process.
        //
        // On a mismatch we still reap the dead/hung child (kill + wait) and
        // remove its socket before returning the error, so we don't leave a
        // zombie process or a stale socket behind. The mismatch is surfaced
        // by the caller as a `PluginHealthEvent::Disabled` diagnostic.
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
            attempt,
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

/// A plugin that completed the handshake during startup, before the
/// capability gate decides whether its tools and providers are registered.
struct StartedPlugin {
    plugin: Arc<Mutex<SupervisedPlugin>>,
    conn: Arc<IpcPluginConnection>,
    name: String,
    capabilities: ene_plugin_proto::PluginCapabilities,
}

/// Pure-function form of the per-restart backoff delay.
fn delay_for_restart(restart_count: usize) -> Duration {
    let delay_ms = BASE_DELAY_MS.saturating_mul(2u64.saturating_pow(restart_count as u32));
    Duration::from_millis(delay_ms.min(MAX_DELAY_MS))
}

/// Sleeps for the restart backoff appropriate for `restart_count`.
///
/// Extracted so tests can substitute a controllable stand-in for the real
/// sleep; production simply awaits `tokio::time::sleep`.
async fn backoff_before_restart(restart_count: usize) {
    tokio::time::sleep(delay_for_restart(restart_count)).await;
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

    // Per-plugin explicit passthrough, filtered against the denylist.
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
    conn: Arc<IpcPluginConnection>,
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

        let result = self.conn.call_tool(name, arguments, context.cloned()).await;

        if result.is_ok() {
            let mut breaker = self.breaker.lock();
            if breaker.consecutive_failures() != 0 {
                self.emit_health(PluginHealthEvent::CircuitClosed {
                    plugin: self.plugin_name.clone(),
                });
            }
            breaker.record_success();
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
        let outcome = self
            .conn
            .call_tool_deferred(name, arguments, context.cloned())
            .await?;
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
        match self.conn.poll_deferred(task_id).await {
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
        if let Err(e) = self.conn.cancel_deferred(task_id).await {
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
        if let Err(e) = self.conn.set_call_context(ctx).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                error = %e,
                "Failed to set call context"
            );
        }
    }

    async fn approve_permission(&self, request_id: &str) {
        if let Err(e) = self.conn.approve_permission(request_id).await {
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
        if let Err(e) = self.conn.allow_pattern(action, target_pattern).await {
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
        if let Err(e) = self.conn.revoke_pattern(action, target_pattern).await {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %self.plugin_name,
                action = %action,
                error = %e,
                "Failed to revoke pattern"
            );
        }
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        match self.conn.config_schema().await {
            Ok(schema) => schema,
            Err(e) => {
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin = %self.plugin_name,
                    error = %e,
                    "Failed to fetch config schema"
                );
                None
            }
        }
    }

    async fn list_config_options(
        &self,
        path: &str,
    ) -> Result<Vec<ene_plugin_proto::ConfigOption>, PluginHostError> {
        self.conn.list_config_options(path).await
    }

    async fn validate_config(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<Vec<ene_plugin_proto::ConfigFieldError>>, PluginHostError> {
        if !self.conn.supports_validate_config() {
            return Ok(None);
        }
        self.conn.validate_config(value).await.map(Some)
    }

    async fn migrate_config(
        &self,
        from_version: u32,
        value: serde_json::Value,
    ) -> Result<(serde_json::Value, u32), PluginHostError> {
        self.conn.migrate_config(from_version, value).await
    }

    fn take_config_schema_changed(&self) -> Option<(String, Option<serde_json::Value>, u32)> {
        self.conn
            .take_config_schema_changed()
            .map(|(schema, version)| (self.plugin_name.clone(), schema, version))
    }
}

/// The factory aliases below are shared with the runtime health bridge so it
/// can retain factory identity while grouping entries by plugin.
pub type LlmFactoryHandle = Arc<dyn ene_ai::LlmProviderFactory>;

/// LLM provider factories grouped by the plugin that provides them.
pub type LlmFactoriesByPlugin = HashMap<String, Vec<(String, LlmFactoryHandle)>>;

/// Embedding provider factories grouped by the plugin that provides them.
pub type EmbeddingFactoriesByPlugin = HashMap<String, Vec<(String, EmbeddingFactoryHandle)>>;

/// A handle to a plugin-provided embedding factory.
pub type EmbeddingFactoryHandle = Arc<dyn ene_ai::EmbeddingProviderFactory>;

/// A handle to a plugin-provided TTS factory.
pub type TtsFactoryHandle = Arc<dyn ene_ai::TtsProviderFactory>;

/// TTS provider factories grouped by the plugin that provides them.
pub type TtsFactoriesByPlugin = HashMap<String, Vec<(String, TtsFactoryHandle)>>;

/// What [`PluginHostManager::remove_provider_factories`] evicted for one
/// plugin, per modality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderFactoryRemoval {
    /// Number of LLM factories removed.
    pub llm: usize,
    /// Number of embedding factories removed.
    pub embedding: usize,
    /// Number of TTS factories removed.
    pub tts: usize,
}

impl ProviderFactoryRemoval {
    /// Whether any factory was removed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.llm == 0 && self.embedding == 0 && self.tts == 0
    }
}

/// Provider factory handles one plugin contributed, captured by a health
/// bridge from the host generation that emitted the `Disabled` event.
///
/// Eviction is gated on [`Arc::ptr_eq`] identity against these handles, so a
/// stale event delivered after a reconfiguration swapped in a fresh host
/// cannot evict the new host's live factories.
#[derive(Clone, Default)]
pub struct PluginFactoryHandles {
    /// LLM factory handles by provider kind.
    pub llm: Vec<(String, LlmFactoryHandle)>,
    /// Embedding factory handles by provider kind.
    pub embedding: Vec<(String, EmbeddingFactoryHandle)>,
    /// TTS factory handles by provider kind.
    pub tts: Vec<(String, TtsFactoryHandle)>,
}

/// Removes the factory for each kind whose stored `Arc` is the exact handle
/// `expected` names, from both the factory map and its owner map.
fn remove_matching_factories<X: ?Sized>(
    factories: &mut HashMap<String, Arc<X>>,
    factory_plugins: &mut HashMap<String, String>,
    expected: &[(String, Arc<X>)],
) -> usize {
    let mut removed = 0;
    for (kind, handle) in expected {
        if factories
            .get(kind)
            .is_some_and(|stored| Arc::ptr_eq(stored, handle))
        {
            factory_plugins.remove(kind);
            factories.remove(kind);
            removed += 1;
        }
    }
    removed
}

/// The plugin host is the single provider registry: provider creation
/// resolves through its factory maps (which mirror the capability
/// declarations), never through a process-global registry.
#[async_trait]
impl ene_ai::ProviderHost for PluginHostManager {
    async fn create_llm_provider(
        &self,
        kind: &str,
        config: &EneConfig,
        task: &ene_ai::TaskRef,
    ) -> Result<Box<dyn ene_ai::LlmProvider>, ene_ai::LlmProviderError> {
        self.llm_factories
            .get(kind)
            .ok_or_else(|| {
                ene_ai::LlmProviderError::Provider(format!(
                    "No LlmProviderFactory registered for provider kind: '{kind}'"
                ))
            })?
            .create_provider(config, task)
    }

    async fn create_embedding_provider(
        &self,
        kind: &str,
        config: &EneConfig,
    ) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
        self.embedding_factories
            .get(kind)
            .ok_or_else(|| {
                ene_ai::EmbeddingError::Init(format!(
                    "No embedding provider factory registered for provider kind: '{kind}'"
                ))
            })?
            .create_embedding_provider(config)
    }

    async fn create_tts_provider(
        &self,
        kind: &str,
        config: &EneConfig,
    ) -> Result<Box<dyn ene_ai::TtsProvider>, ene_ai::AudioProviderError> {
        self.tts_factories
            .get(kind)
            .ok_or_else(|| {
                ene_ai::AudioProviderError::Provider(format!(
                    "No TtsProviderFactory registered for provider name: '{kind}'"
                ))
            })?
            .create_provider(config)
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
/// - `capabilities.tts_providers` → registered as [`IpcTtsProviderFactory`] entries
///
/// Additionally connects to any MCP servers declared in `plugins.mcp_servers`
/// and includes their tools in [`tool_registries`](Self::tool_registries).
pub struct PluginHostManager {
    supervised: Vec<Arc<Mutex<SupervisedPlugin>>>,
    connections: Vec<Arc<IpcPluginConnection>>,
    /// Plugin names, index-aligned with `connections` — both vectors are
    /// pushed together in [`start`](Self::start). Stored separately from
    /// `supervised` so config pushes can identify a connection without
    /// locking its `SupervisedPlugin` (a lock a health probe or restart
    /// may be holding).
    names: Vec<String>,
    tool_registries: Vec<Arc<dyn ToolRegistry>>,
    llm_factories: HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>>,
    llm_factory_plugins: HashMap<String, String>,
    embedding_factories: HashMap<String, Arc<dyn ene_ai::EmbeddingProviderFactory>>,
    embedding_factory_plugins: HashMap<String, String>,
    tts_factories: HashMap<String, Arc<dyn ene_ai::TtsProviderFactory>>,
    tts_factory_plugins: HashMap<String, String>,
    /// Credential declarations parsed from each plugin's `x-ene-credentials`
    /// schema block at startup. Populated by [`start`](Self::start) alongside
    /// connection registration; consumed by the credential service for
    /// request-time scope enforcement.
    credential_registry: CredentialRegistry,
    /// Capability declarations indexed from each plugin's handshake, after
    /// the startup gate removed plugins with unmet hard requirements.
    /// Resolves `requires` to provider plugins; the future capability
    /// mediation service uses it as its ACL source.
    capability_registry: CapabilityRegistry,
    /// One supervisor task per supervised plugin. Each task independently
    /// pings and restarts only its own plugin, so one plugin's restart
    /// backoff or reconnect can never stall monitoring of the others.
    /// Empty when health probes are disabled or no plugins are supervised.
    health_tasks: Vec<tokio::task::JoinHandle<()>>,
    health_rx: Option<mpsc::UnboundedReceiver<PluginHealthEvent>>,
    /// When true (default), Drop will attempt a best-effort graceful shutdown
    /// by killing child processes with a brief wait for reaping.
    shutdown_on_drop: bool,
}

impl Drop for PluginHostManager {
    fn drop(&mut self) {
        for task in self.health_tasks.drain(..) {
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
                names: Vec::new(),
                tool_registries: Vec::new(),
                llm_factories: HashMap::new(),
                llm_factory_plugins: HashMap::new(),
                embedding_factories: HashMap::new(),
                embedding_factory_plugins: HashMap::new(),
                tts_factories: HashMap::new(),
                tts_factory_plugins: HashMap::new(),
                credential_registry: CredentialRegistry::new(),
                capability_registry: CapabilityRegistry::new(),
                health_tasks: Vec::new(),
                health_rx: None,
                shutdown_on_drop: true,
            });
        }

        let (health_tx, health_rx) = mpsc::unbounded_channel::<PluginHealthEvent>();

        let mut supervised: Vec<Arc<Mutex<SupervisedPlugin>>> = Vec::new();
        let mut connections: Vec<Arc<IpcPluginConnection>> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut tool_registries: Vec<Arc<dyn ToolRegistry>> = Vec::new();
        let mut llm_factories: HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>> =
            HashMap::new();
        let mut llm_factory_plugins: HashMap<String, String> = HashMap::new();
        let mut embedding_factories: HashMap<String, Arc<dyn ene_ai::EmbeddingProviderFactory>> =
            HashMap::new();
        let mut embedding_factory_plugins: HashMap<String, String> = HashMap::new();
        let mut tts_factories: HashMap<String, Arc<dyn ene_ai::TtsProviderFactory>> =
            HashMap::new();
        let mut tts_factory_plugins: HashMap<String, String> = HashMap::new();
        // (plugin name, fetched schema) pairs; the schemas are registered after
        // `Self` exists so the registration step is a `&self` method that unit
        // tests can drive without a plugin process.
        let mut credential_schemas: Vec<(String, Option<serde_json::Value>)> = Vec::new();

        std::fs::create_dir_all(ene_config::plugin_socket_dir()).map_err(|e| {
            PluginHostError::ExecutionFailed {
                message: format!("Failed to create plugin socket dir: {e}"),
            }
        })?;

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

        // Pass 1: handshake every plugin and collect its declarations. Tool
        // and provider registration is deferred to pass 2 so the capability
        // gate can decide which plugins are registered at all.
        let mut started: Vec<StartedPlugin> = Vec::new();
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

            match Self::start_plugin(
                name,
                entry.delivered_config(name),
                entry.delivered_profiles(),
                entry.checksum.clone(),
                entry.env_passthrough.clone(),
                db_tokens.get(name).cloned(),
                Duration::from_millis(plugin_config.handshake_timeout_ms),
                plugin_config.max_concurrent,
            )
            .await
            {
                Ok((plugin, conn, tofu_checksum)) => {
                    if let Some(checksum) = tofu_checksum {
                        checksums_to_record.insert(name.clone(), checksum);
                    }
                    started.push(StartedPlugin {
                        capabilities: conn.capabilities(),
                        plugin,
                        conn,
                        name: name.clone(),
                    });
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

        // Capability gate: a plugin whose hard requirements have no provider
        // is not registered at all (tools, providers, supervision). The
        // fixpoint in `evaluate_capability_gate` also disables consumers of
        // disabled providers, so the registry handed to the manager only ever
        // resolves against plugins that actually started.
        let declarations: Vec<CapabilityDeclaration> = started
            .iter()
            .map(|started_plugin| CapabilityDeclaration {
                plugin: started_plugin.name.clone(),
                capabilities: started_plugin.capabilities.clone(),
            })
            .collect();
        let (capability_registry, disabled_by_requirements) =
            evaluate_capability_gate(&declarations);
        for plugin_name in &disabled_by_requirements {
            let requirements: Vec<String> = capability_registry
                .unmet_hard_requirements(plugin_name)
                .iter()
                .map(ToString::to_string)
                .collect();
            drop(health_tx.send(PluginHealthEvent::RequirementsUnmet {
                plugin: plugin_name.clone(),
                requirements: requirements.clone(),
            }));
            tracing::error!(
                component = "PluginHostManager",
                plugin = %plugin_name,
                requirements = ?requirements,
                "Disabling plugin: hard capability requirements are unmet"
            );
        }
        for started_plugin in &started {
            if disabled_by_requirements.contains(&started_plugin.name) {
                continue;
            }
            let requirements: Vec<String> = capability_registry
                .unmet_soft_requirements(&started_plugin.name)
                .iter()
                .map(ToString::to_string)
                .collect();
            if !requirements.is_empty() {
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin = %started_plugin.name,
                    requirements = ?requirements,
                    "Plugin started with unmet soft capability requirements; fallback is expected"
                );
            }
        }

        // Pass 2: register the tools and providers of every plugin that
        // passed the gate.
        for started_plugin in &started {
            if disabled_by_requirements.contains(&started_plugin.name) {
                continue;
            }
            let name = &started_plugin.name;
            let conn = &started_plugin.conn;

            // The plugin config blob is opaque to the host; log only a
            // redacted view so a secret (e.g. an inline `api_key`) can
            // never appear in the log stream. Redact against the
            // plugin's own schema when it advertises one — a custom
            // secret key name (`x-ene-secret: true`) is only caught by
            // the schema-aware pass — and fall back to the
            // schema-independent redaction otherwise.
            //
            // The schema is fetched exactly once and also feeds the
            // credential declaration registration below, so a single
            // IPC round-trip serves both.
            let schema = match conn.config_schema().await {
                Ok(schema) => schema,
                Err(e) => {
                    tracing::warn!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Failed to fetch config schema"
                    );
                    None
                }
            };

            if let Some(config) = plugin_config
                .list
                .get(name)
                .and_then(|entry| entry.delivered_config(name))
            {
                let redacted = match &schema {
                    Some(schema) => crate::redact::redact_config(&config, Some(schema)),
                    None => crate::redact::redact_config_unschematized(&config),
                };
                tracing::debug!(
                    component = "PluginHostManager",
                    plugin = %name,
                    config = %redacted,
                    "Starting plugin with configuration"
                );
            }

            // Collect the schema for credential declaration
            // registration, applied after the manager is assembled
            // (see `register_credential_schema`). Invalid entries are
            // warned about and dropped there; the plugin itself always
            // keeps running (a bad declaration only loses that
            // declaration).
            credential_schemas.push((name.clone(), schema));

            let caps = &started_plugin.capabilities;

            if caps.tools > 0 {
                // Retry once on failure; if it still fails, skip
                // registering the tool registry (don't silently
                // register empty tools). The plugin process itself is
                // still supervised so any LLM providers it offers keep
                // working.
                let tools = match conn.list_tools().await {
                    Ok(tools) => Some(tools),
                    Err(e) => {
                        tracing::warn!(
                            component = "PluginHostManager",
                            plugin = %name,
                            error = %e,
                            "Failed to list tools for plugin; retrying once"
                        );
                        match conn.list_tools().await {
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
                        conn: Arc::clone(conn),
                        tools,
                        breaker: parking_lot::Mutex::new(CircuitBreaker::default()),
                        health_tx: Some(health_tx.clone()),
                    };
                    tool_registries.push(Arc::new(registry));
                }
            }

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
                    Arc::clone(conn),
                    name.clone(),
                    is_builtin_plugin(name),
                    spec.context_window,
                    spec.concurrency,
                );
                llm_factories.insert(
                    spec.kind.clone(),
                    Arc::new(factory) as Arc<dyn ene_ai::LlmProviderFactory>,
                );
                llm_factory_plugins.insert(spec.kind.clone(), name.clone());
            }

            for kind in &caps.embed_providers {
                if embedding_factories.contains_key(kind) {
                    tracing::warn!(
                        component = "PluginHostManager",
                        plugin = %name,
                        kind = %kind,
                        "Duplicate embedding provider kind; skipping"
                    );
                    continue;
                }
                let factory = IpcEmbeddingProviderFactory::new(
                    kind.clone(),
                    Arc::clone(conn),
                    name.clone(),
                    is_builtin_plugin(name),
                );
                embedding_factories.insert(
                    kind.clone(),
                    Arc::new(factory) as Arc<dyn ene_ai::EmbeddingProviderFactory>,
                );
                embedding_factory_plugins.insert(kind.clone(), name.clone());
            }

            for spec in &caps.tts_providers {
                if tts_factories.contains_key(&spec.kind) {
                    tracing::warn!(
                        component = "PluginHostManager",
                        plugin = %name,
                        kind = %spec.kind,
                        "Duplicate TTS provider kind; skipping"
                    );
                    continue;
                }
                let factory = IpcTtsProviderFactory::new(
                    spec.kind.clone(),
                    Arc::clone(conn),
                    name.clone(),
                    spec.concurrency,
                );
                tts_factories.insert(
                    spec.kind.clone(),
                    Arc::new(factory) as Arc<dyn ene_ai::TtsProviderFactory>,
                );
                tts_factory_plugins.insert(spec.kind.clone(), name.clone());
            }

            supervised.push(Arc::clone(&started_plugin.plugin));
            connections.push(Arc::clone(conn));
            names.push(name.clone());
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

        // Connect to configured MCP servers. Each server gets its own
        // McpToolRegistry, which is added to the tool registries regardless of
        // whether the connection succeeds (a failed server simply advertises no
        // tools). Server names are used verbatim — no charset validation — so
        // hyphenated and other names connect identically.
        if !plugin_config.mcp_servers.is_empty() {
            for server in &plugin_config.mcp_servers {
                if !server.enabled {
                    continue;
                }

                let registry = Arc::new(McpToolRegistry::new());

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
                        // Log scheme/host/port only — the URL may embed userinfo
                        // credentials (`https://user:token@host/sse`).
                        let (scheme, host, port) = redacted_endpoint(url);
                        tracing::info!(
                            component = "PluginHostManager",
                            server = %server.name,
                            scheme = %scheme,
                            host = %host,
                            port = ?port,
                            "Connecting to MCP server via streamable HTTP transport"
                        );
                        if let Err(err) = registry
                            .connect_http(
                                &server.name,
                                url,
                                auth_header.as_deref(),
                                plugin_config.mcp_allow_insecure_urls,
                            )
                            .await
                        {
                            // A rejected server is a configuration error the user
                            // must act on — its tools silently disappear — so log
                            // at error level rather than warn.
                            tracing::error!(
                                component = "PluginHostManager",
                                server = %server.name,
                                scheme = %scheme,
                                host = %host,
                                port = ?port,
                                error = %err,
                                "MCP HTTP connection failed"
                            );
                        }
                    }
                }

                tool_registries.push(registry);
            }
        }

        // Spawn one independent supervisor task per plugin via the
        // [`spawn_supervisors`] helper (tested directly for its
        // one-handle-per-plugin wiring). Each task pings and restarts only its
        // own plugin, so one plugin's restart backoff (up to 30 s) or a slow
        // reconnect can never stall the monitoring of any other plugin.
        // The task count is proportional to the plugin count — a handful in
        // practice.
        let health_interval = Duration::from_millis(plugin_config.health_interval_ms);
        let health_tasks = spawn_supervisors(
            health_interval,
            &supervised,
            &connections,
            &health_tx,
            backoff_before_restart,
        );

        let manager = Self {
            supervised,
            connections,
            names,
            tool_registries,
            llm_factories,
            llm_factory_plugins,
            embedding_factories,
            embedding_factory_plugins,
            tts_factories,
            tts_factory_plugins,
            credential_registry: CredentialRegistry::new(),
            capability_registry,
            health_tasks,
            health_rx: Some(health_rx),
            shutdown_on_drop: true,
        };
        for (plugin, schema) in credential_schemas {
            manager.register_credential_schema(&plugin, schema.as_ref());
        }
        Ok(manager)
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

    /// Removes the provider factories `handles` identifies.
    ///
    /// Called when a plugin is permanently disabled: without eviction, a
    /// lookup by kind would keep selecting a factory whose IPC connection
    /// points at a dead process. Removal is identity-gated on the handles a
    /// health bridge captured from this host generation, so a stale event
    /// cannot evict a replacement host's factories. Returns what was removed
    /// so callers can rebuild long-lived provider instances (TTS) accordingly.
    pub fn remove_provider_factories_if_match(
        &mut self,
        handles: &PluginFactoryHandles,
    ) -> ProviderFactoryRemoval {
        ProviderFactoryRemoval {
            llm: remove_matching_factories(
                &mut self.llm_factories,
                &mut self.llm_factory_plugins,
                &handles.llm,
            ),
            embedding: remove_matching_factories(
                &mut self.embedding_factories,
                &mut self.embedding_factory_plugins,
                &handles.embedding,
            ),
            tts: remove_matching_factories(
                &mut self.tts_factories,
                &mut self.tts_factory_plugins,
                &handles.tts,
            ),
        }
    }

    /// Returns the embedding provider factories contributed by plugins,
    /// keyed by provider kind.
    pub fn embedding_factories(
        &self,
    ) -> &HashMap<String, Arc<dyn ene_ai::EmbeddingProviderFactory>> {
        &self.embedding_factories
    }

    /// Returns the embedding factories grouped by the plugin that provides
    /// them, mirroring [`llm_factories_by_plugin`](Self::llm_factories_by_plugin).
    pub fn embedding_factories_by_plugin(&self) -> EmbeddingFactoriesByPlugin {
        let mut grouped = EmbeddingFactoriesByPlugin::new();
        for (kind, factory) in &self.embedding_factories {
            if let Some(plugin) = self.embedding_factory_plugins.get(kind) {
                grouped
                    .entry(plugin.clone())
                    .or_default()
                    .push((kind.clone(), Arc::clone(factory)));
            }
        }
        grouped
    }

    /// Returns the TTS provider factories contributed by plugins, keyed by
    /// provider kind.
    pub fn tts_factories(&self) -> &HashMap<String, Arc<dyn ene_ai::TtsProviderFactory>> {
        &self.tts_factories
    }

    /// Returns the TTS factories grouped by the plugin that provides them,
    /// mirroring [`llm_factories_by_plugin`](Self::llm_factories_by_plugin).
    pub fn tts_factories_by_plugin(&self) -> TtsFactoriesByPlugin {
        let mut grouped = TtsFactoriesByPlugin::new();
        for (kind, factory) in &self.tts_factories {
            if let Some(plugin) = self.tts_factory_plugins.get(kind) {
                grouped
                    .entry(plugin.clone())
                    .or_default()
                    .push((kind.clone(), Arc::clone(factory)));
            }
        }
        grouped
    }

    /// Returns the LLM factories grouped by the plugin that provides them.
    ///
    /// The factory handles are included so consumers can deregister only the
    /// exact entries owned by this host, rather than removing a replacement
    /// installed by another runtime handle.
    pub fn llm_factories_by_plugin(&self) -> LlmFactoriesByPlugin {
        let mut grouped = LlmFactoriesByPlugin::new();
        for (kind, factory) in &self.llm_factories {
            if let Some(plugin) = self.llm_factory_plugins.get(kind) {
                grouped
                    .entry(plugin.clone())
                    .or_default()
                    .push((kind.clone(), Arc::clone(factory)));
            }
        }
        grouped
    }

    /// Takes ownership of the health-event receiver.
    ///
    /// The runtime calls this once after startup to bridge plugin health
    /// events into its diagnostics channel. Returns `None` on subsequent
    /// calls.
    pub fn take_health_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<PluginHealthEvent>> {
        self.health_rx.take()
    }

    /// Returns the capability registry built from the handshake declarations
    /// of every started plugin (after the startup gate removed plugins with
    /// unmet hard requirements).
    #[must_use]
    pub fn capability_registry(&self) -> &CapabilityRegistry {
        &self.capability_registry
    }

    /// Registers the credential declarations parsed from `schema` for
    /// `plugin`, replacing any previous registration.
    ///
    /// [`start`](Self::start) applies this to every started plugin with the
    /// schema fetched for config redaction; the credential service reads the
    /// result via [`Self::resolve_credential_scope`]. Kept as a `&self` method
    /// so the register→resolve path is testable without a plugin process.
    fn register_credential_schema(&self, plugin: &str, schema: Option<&serde_json::Value>) {
        self.credential_registry
            .register_from_schema(plugin, schema);
    }

    /// Returns the credential declarations registered for `plugin`.
    ///
    /// Empty when the plugin declared none or its schema could not be
    /// fetched at startup.
    #[must_use]
    pub fn credential_declarations(&self, plugin: &str) -> Vec<CredentialDeclaration> {
        self.credential_registry.declarations(plugin)
    }

    /// Resolves whether `plugin` may access credential `id`, per the
    /// declarations registered at startup.
    #[must_use]
    pub fn resolve_credential_scope(&self, plugin: &str, id: &CredentialId) -> ScopeDecision {
        self.credential_registry.resolve_scope(plugin, id)
    }

    /// Controls whether Drop attempts best-effort shutdown.
    /// Set to false if the caller will handle shutdown explicitly.
    pub fn set_shutdown_on_drop(&mut self, value: bool) {
        self.shutdown_on_drop = value;
    }

    /// Pushes updated config/profiles to live connections whose plugin names
    /// appear in `updates`.
    ///
    /// Each connection updates its stored blobs first so a later reconnect
    /// handshake uses the fresh values, then sends `SetConfig` when the
    /// negotiated protocol version supports it (otherwise the connection
    /// reports [`crate::ipc_plugin::SetConfigOutcome::CachedOnly`]). Failures
    /// are logged and do not abort the remaining updates.
    pub async fn apply_plugin_configs(
        &self,
        updates: &HashMap<String, (Option<serde_json::Value>, Option<serde_json::Value>)>,
    ) {
        if updates.is_empty() {
            return;
        }
        for (name, conn) in self.names.iter().zip(self.connections.iter()) {
            let Some((config, profiles)) = updates.get(name) else {
                continue;
            };
            match conn.set_config(config.clone(), profiles.clone()).await {
                Ok(outcome) => {
                    tracing::debug!(
                        component = "PluginHostManager",
                        plugin = %name,
                        outcome = ?outcome,
                        "SetConfig delivered"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Failed to push SetConfig to live plugin"
                    );
                }
            }
        }
    }

    /// Sends a graceful `Shutdown` to all plugins and kills the processes.
    pub async fn shutdown(&mut self) {
        // Abort every per-plugin supervisor first so none can race with
        // shutdown (e.g. restart a plugin we are about to kill).
        for task in self.health_tasks.drain(..) {
            task.abort();
        }
        for conn in &self.connections {
            conn.shutdown().await;
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
        plugin_profiles: Option<serde_json::Value>,
        expected_checksum: Option<String>,
        env_passthrough: Vec<String>,
        db_token: Option<String>,
        handshake_timeout: Duration,
        max_concurrent: usize,
    ) -> Result<
        (
            Arc<Mutex<SupervisedPlugin>>,
            Arc<IpcPluginConnection>,
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
            let host_socket = {
                #[cfg(unix)]
                {
                    ene_config::paths::tool_socket_dir().join("ene-host-service.sock")
                }
                #[cfg(windows)]
                {
                    PathBuf::from(r"\\.\pipe\ene-host-service")
                }
                #[cfg(not(any(unix, windows)))]
                {
                    ene_config::paths::tool_socket_dir().join("ene-host-service.sock")
                }
            };
            let host_socket = host_socket.to_string_lossy().to_string();
            sandbox.host_service_socket = Some(host_socket.clone());
            // Compatibility alias: plugins that only read `db_socket` still
            // reach the shared host-service endpoint.
            sandbox.db_socket = Some(host_socket);
        }

        let mut child = build_plugin_command(&binary_path, &socket_path, &env_passthrough)
            .spawn()
            .map_err(|e| PluginHostError::SpawnFailed {
                name: name.to_string(),
                reason: e.to_string(),
            })?;

        let conn = match IpcPluginConnection::connect(
            &socket_path,
            sandbox.clone(),
            plugin_config.clone(),
            plugin_profiles,
            handshake_timeout,
            max_concurrent,
        )
        .await
        {
            Ok(conn) => conn,
            Err(e) => {
                // A handshake failure (e.g. the handshake timeout firing on a
                // stalling plugin) leaves the just-spawned child running;
                // dropping a std `Child` does not kill it. Kill and reap it so
                // a wedged plugin does not leak a process and its socket file
                // across launches.
                drop(child.kill());
                drop(child.wait());
                ene_plugin_proto::cleanup_path(&socket_path);
                return Err(e);
            }
        };

        // The pinned checksum for restart-time verification: the configured
        // checksum when present, otherwise the trust-on-first-use checksum
        // just recorded. Either way the binary that was verified at startup
        // is pinned and re-verified on every restart.
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
            env_passthrough,
            restart_times: VecDeque::new(),
            disabled: false,
        };

        Ok((Arc::new(Mutex::new(plugin)), Arc::new(conn), tofu_checksum))
    }

    /// Test-only manager with no plugins and no health bridge.
    #[cfg(test)]
    fn test_instance() -> Self {
        Self {
            supervised: Vec::new(),
            connections: Vec::new(),
            names: Vec::new(),
            tool_registries: Vec::new(),
            llm_factories: HashMap::new(),
            llm_factory_plugins: HashMap::new(),
            embedding_factories: HashMap::new(),
            embedding_factory_plugins: HashMap::new(),
            tts_factories: HashMap::new(),
            tts_factory_plugins: HashMap::new(),
            credential_registry: CredentialRegistry::new(),
            capability_registry: CapabilityRegistry::new(),
            health_tasks: Vec::new(),
            health_rx: None,
            shutdown_on_drop: false,
        }
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

/// The plugin binaries that ship with Ene as trusted built-ins.
///
/// Compiled into the host rather than discovered from the filesystem: in
/// debug builds [`builtin_plugins_dir`](ene_config::builtin_plugins_dir)
/// resolves to the directory of the running executable (`target/debug/...`),
/// so a path-existence check would let any `ene-plugin-*` binary dropped
/// there masquerade as a trusted built-in and pass the credential trust gate
/// in [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory).
/// Matching against a fixed list keeps that gate independent of whatever
/// happens to be on disk.
///
/// Keep in sync with the `plugins/` directory, the default `plugins.list` in
/// [`config`](crate::config), and the built-in catalog in
/// `docs/concepts/plugins-and-mcp.md`.
pub(crate) const BUILTIN_PLUGIN_NAMES: &[&str] = &[
    "anthropic",
    "app",
    "browser",
    "calc",
    "calendar",
    "counter",
    "edge-tts",
    "elevenlabs",
    "fs",
    "geo",
    "git",
    "homeassistant",
    "kokoro",
    "llama-cpp",
    "openai",
    "openai-tts",
    "random",
    "utility",
    "voicevox",
    "web",
];

/// Returns `true` when the plugin is one of the trusted built-ins that ship
/// with Ene. Used by the API key trust gate: only builtin or explicitly
/// configured plugins receive resolved credentials.
///
/// This matches against the compiled-in [`BUILTIN_PLUGIN_NAMES`] list rather
/// than probing the filesystem (see that constant's docs for why).
fn is_builtin_plugin(name: &str) -> bool {
    BUILTIN_PLUGIN_NAMES.contains(&name)
}

/// Finds the binary path for a plugin by name, searching builtin and user
/// directories with both `ene-plugin-{name}` and `{name}` naming conventions.
///
/// The search order is fixed and **the builtin directory takes priority**:
///
/// 1. `<builtin>/ene-plugin-{name}`
/// 2. `<builtin>/{name}`
/// 3. `<user>/ene-plugin-{name}`
/// 4. `<user>/{name}`
///
/// The first candidate that exists as a file wins. Because builtin entries
/// come first, a user plugin placed under
/// [`user_plugins_dir`](ene_config::user_plugins_dir) can **never** shadow a
/// built-in of the same name — the shipped binary always runs. This is the
/// deliberate, security-conservative choice (a user drop-in cannot silently
/// replace a trusted built-in), but it also means a user who places a binary
/// with the same name as a built-in will see the built-in run instead of
/// theirs; pick a distinct name to override behavior.
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

/// Spawns one independent [`supervise_plugin`] task per supervised plugin and
/// returns their join handles.
///
/// Exactly one handle is returned per `(supervised, connection)` pair — the
/// wiring that keeps one plugin's restart backoff from stalling any other's
/// monitoring. When `interval` is zero or there are no supervised
/// plugins, health probing is skipped entirely and an empty list is returned.
///
/// `backoff_sleep` is passed through to [`supervise_plugin`]; production
/// passes [`backoff_before_restart`], tests inject a controllable stand-in.
/// It must be `Copy` because it is handed to every spawned task, and
/// `Send + 'static` (along with a `Send` future) because the tasks run on the
/// Tokio runtime. It is generic over the returned future so no boxing (and no
/// extra dependency) is needed.
fn spawn_supervisors<F>(
    interval: Duration,
    supervised: &[Arc<Mutex<SupervisedPlugin>>],
    connections: &[Arc<IpcPluginConnection>],
    health_tx: &mpsc::UnboundedSender<PluginHealthEvent>,
    backoff_sleep: impl Fn(usize) -> F + Copy + Send + 'static,
) -> Vec<tokio::task::JoinHandle<()>>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if supervised.is_empty() || interval.is_zero() {
        if interval.is_zero() && !supervised.is_empty() {
            tracing::info!(
                component = "PluginHostManager",
                "Health probes disabled by configuration (health_interval_ms = 0)"
            );
        }
        return Vec::new();
    }

    supervised
        .iter()
        .zip(connections.iter())
        .map(|(plugin, conn)| {
            tokio::spawn(supervise_plugin(
                interval,
                Arc::clone(plugin),
                Arc::clone(conn),
                health_tx.clone(),
                backoff_sleep,
            ))
        })
        .collect()
}

/// Per-plugin health supervisor: periodically pings a single plugin and
/// restarts it when dead or unresponsive, emitting health events.
///
/// One of these tasks runs per supervised plugin. Because each task
/// owns exactly one plugin, its restart backoff (`delay_for_restart`, up to
/// 30 s) and any slow reconnect delay only *its own* next probe — they can
/// never stall the monitoring of any other plugin, as a single shared loop
/// would. The task runs until aborted by
/// [`PluginHostManager::shutdown`] or drop — except for permanently disabled
/// plugins (restart budget exhausted or checksum mismatch), for which the
/// task exits as soon as it observes `disabled`, releasing its refs.
///
/// `backoff_sleep` performs the restart backoff for a given restart count.
/// Production passes [`backoff_before_restart`]; tests inject a controllable
/// stand-in so the backoff can be observed without real delays. It is generic
/// over the returned future so no boxing (and no extra dependency) is needed.
///
/// Restart budget is a rolling window enforced solely inside
/// [`SupervisedPlugin::restart`]. Healthy probes do not clear that window —
/// only elapsed time does — so a crash loop that recovers just long enough to
/// answer a ping still accumulates toward disablement.
async fn supervise_plugin<F>(
    interval: Duration,
    plugin: Arc<Mutex<SupervisedPlugin>>,
    conn: Arc<IpcPluginConnection>,
    health_tx: mpsc::UnboundedSender<PluginHealthEvent>,
    backoff_sleep: impl Fn(usize) -> F,
) where
    F: std::future::Future<Output = ()>,
{
    let mut ticker = tokio::time::interval(interval);
    // Skip the immediate first tick.
    ticker.tick().await;
    loop {
        ticker.tick().await;

        // Permanently disabled plugins (restart budget exhausted or a
        // restart-time checksum mismatch) are left stopped. The supervisor
        // exits here rather than waking each interval only to `continue`,
        // releasing its `Arc` refs so the plugin and connection can be dropped.
        if plugin.lock().await.disabled {
            break;
        }

        // Ping without any connection lock: `ping` takes `&self` and the
        // writer lock is held only for the frame write, so a probe is never
        // queued behind an in-flight tool call.
        let ping_ok = conn.ping().await.is_ok();
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

        // Backoff from the current window length; the budget limit itself is
        // enforced only inside `restart()` so there is a single comparison.
        let recent = {
            let mut p = plugin.lock().await;
            p.recent_restart_count()
        };
        drop(health_tx.send(PluginHealthEvent::Restarting {
            plugin: name.clone(),
            attempt: recent.saturating_add(1),
        }));

        // The backoff sleep delays only this plugin's supervisor; every other
        // plugin keeps being probed on schedule by its own task.
        backoff_sleep(recent).await;

        let mut p = plugin.lock().await;
        if let Err(e) = p.restart() {
            // A checksum mismatch means the on-disk binary changed since
            // it was last verified. Do NOT silently retry: disable the
            // plugin and tell the user via a diagnostic.
            //
            // `ExecutionFailed` from `restart()` is the rolling-window budget
            // exhaustion path — the sole limit check lives there.
            match &e {
                PluginHostError::ChecksumMismatch { .. } => {
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
                }
                PluginHostError::ExecutionFailed { .. } => {
                    tracing::error!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Plugin exceeded max restarts; disabled"
                    );
                    p.disabled = true;
                    drop(health_tx.send(PluginHealthEvent::Disabled {
                        plugin: name,
                        reason: crate::health::DisabledReason::RestartBudgetExhausted,
                    }));
                }
                _ => {
                    tracing::error!(
                        component = "PluginHostManager",
                        plugin = %name,
                        error = %e,
                        "Failed to restart plugin"
                    );
                }
            }
            continue;
        }
        drop(p);

        // Reconnect the shared connection in place: `reconnect` takes
        // `&self`, re-performs the handshake on the stored socket path, and
        // swaps the writer/reader under their own locks, so every caller
        // sharing this `Arc` sees the fresh transport. A slow or
        // failing reconnect delays only this plugin's supervisor.
        match conn.reconnect().await {
            Ok(()) => {
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

/// Computes the SHA-256 checksum of a plugin binary, hex-encoded.
///
/// The file is hashed in fixed-size 64KiB chunks read directly from the
/// [`std::fs::File`] so the entire binary is never held in memory at once —
/// llama-linked plugin binaries can exceed 100MB, and this runs on every
/// restart. A `BufReader` is deliberately not used: its 8KiB internal
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
/// (configured or trust-on-first-use), so verification never fails open.
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
    clippy::expect_used,
    reason = "tests use unwrap/expect for concise failure messages"
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
    fn is_builtin_plugin_matches_compiled_in_list() {
        // Every shipped built-in is trusted...
        for &name in BUILTIN_PLUGIN_NAMES {
            assert!(is_builtin_plugin(name), "{name} must be a built-in");
        }
        // ...and the list is exactly the shipped set (no accidental drift).
        assert_eq!(
            BUILTIN_PLUGIN_NAMES,
            &[
                "anthropic",
                "app",
                "browser",
                "calc",
                "calendar",
                "counter",
                "edge-tts",
                "elevenlabs",
                "fs",
                "geo",
                "git",
                "homeassistant",
                "kokoro",
                "llama-cpp",
                "openai",
                "openai-tts",
                "random",
                "utility",
                "voicevox",
                "web"
            ]
        );

        // An arbitrary binary dropped into the plugins directory must NOT be
        // treated as a built-in: trust comes from the compiled-in list, not
        // from a file existing on disk.
        assert!(!is_builtin_plugin("evil"));
        assert!(!is_builtin_plugin("ene-plugin-evil"));
        assert!(!is_builtin_plugin(""));
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
    fn credential_registration_wiring_resolves_scope() {
        // Drives the exact registration step `start()` applies per plugin
        // (schema → `register_credential_schema` → `resolve_credential_scope`)
        // without spawning a plugin process.
        let manager = PluginHostManager::test_instance();
        let schema = serde_json::json!({
            "x-ene-credentials": [
                { "id": "anthropic", "kind": "api_key" },
                { "id": "secret.key", "kind": "api_key", "shared": false }
            ]
        });
        manager.register_credential_schema("plugin-a", Some(&schema));

        let anthropic = CredentialId::try_new("anthropic").unwrap();
        assert_eq!(
            manager.resolve_credential_scope("plugin-a", &anthropic),
            ScopeDecision::Allowed {
                storage_key: "anthropic".to_string()
            }
        );
        let secret = CredentialId::try_new("secret.key").unwrap();
        assert_eq!(
            manager.resolve_credential_scope("plugin-a", &secret),
            ScopeDecision::Allowed {
                storage_key: "plugin-a:secret.key".to_string()
            }
        );
        // A plugin that never registered is denied even when another declared
        // the same id.
        assert_eq!(
            manager.resolve_credential_scope("plugin-b", &anthropic),
            ScopeDecision::Undeclared
        );
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

        let mut hasher = sha2::Sha256::new();
        hasher.update(b"fake plugin binary contents");
        let real = hex::encode(hasher.finalize());

        // The streaming helper must produce the same digest as the in-memory
        // reference hash (it reads in chunks, never the whole file).
        assert_eq!(compute_binary_checksum("fake", &bin_path).unwrap(), real);

        assert!(verify_and_record_checksum("fake", &bin_path, Some(&real)).is_ok());
        assert_eq!(
            verify_and_record_checksum("fake", &bin_path, Some(&real)).unwrap(),
            None
        );

        // Case-insensitive comparison: uppercase hex also matches.
        let upper = real.to_ascii_uppercase();
        assert!(verify_and_record_checksum("fake", &bin_path, Some(&upper)).is_ok());

        let result = verify_and_record_checksum("fake", &bin_path, Some("deadbeef"));
        assert!(matches!(
            result,
            Err(PluginHostError::ChecksumMismatch { .. })
        ));

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
            env_passthrough: Vec::new(),
            restart_times: VecDeque::new(),
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

        assert!(plugin.restart().is_ok());
        assert_eq!(plugin.recent_restart_count(), 1);

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

        let real = compute_binary_checksum("fake", &binary_path).unwrap();
        let mut plugin = supervised_plugin_with(binary_path.clone(), PinnedChecksum::Tofu(real));

        {
            let mut f = std::fs::File::create(&binary_path).unwrap();
            f.write_all(b"tampered binary contents that differ from the original")
                .unwrap();
        }

        let result = plugin.restart();
        assert!(matches!(
            result,
            Err(PluginHostError::ChecksumMismatch { .. })
        ));
        // ...and the attempt consumed one unit of restart budget. Verification
        // happens before spawning, so the tampered binary was never exec'd; the
        // dead/hung child is reaped on the mismatch path (no zombie left).
        assert_eq!(plugin.recent_restart_count(), 1);

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    #[cfg(unix)]
    fn restart_budget_caps_within_window() {
        let dir = temp_plugin_dir("budget-cap");
        let binary_path = dir.join("ene-plugin-fake");
        write_dummy_script(&binary_path);

        let real = compute_binary_checksum("fake", &binary_path).unwrap();
        let mut plugin = supervised_plugin_with(binary_path, PinnedChecksum::Tofu(real));

        for _ in 0..MAX_RESTARTS {
            assert!(plugin.restart().is_ok());
        }
        assert_eq!(plugin.recent_restart_count(), MAX_RESTARTS);
        assert!(matches!(
            plugin.restart(),
            Err(PluginHostError::ExecutionFailed { .. })
        ));
        // Failed budget check must not push another timestamp.
        assert_eq!(plugin.recent_restart_count(), MAX_RESTARTS);

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    #[cfg(unix)]
    fn restart_budget_prunes_expired_entries() {
        let dir = temp_plugin_dir("budget-prune");
        let binary_path = dir.join("ene-plugin-fake");
        write_dummy_script(&binary_path);

        let real = compute_binary_checksum("fake", &binary_path).unwrap();
        let mut plugin = supervised_plugin_with(binary_path, PinnedChecksum::Tofu(real));

        let expired = Instant::now()
            .checked_sub(RESTART_WINDOW + Duration::from_secs(1))
            .expect("restart window fits in Instant");
        for _ in 0..MAX_RESTARTS {
            plugin.restart_times.push_back(expired);
        }
        assert_eq!(plugin.recent_restart_count(), 0);
        assert!(plugin.restart().is_ok());
        assert_eq!(plugin.recent_restart_count(), 1);

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn build_plugin_command_clears_environment() {
        use std::ffi::OsStr;

        let binary = std::path::Path::new("/usr/bin/env");
        let socket = std::path::Path::new("/tmp/test.sock");

        let cmd = build_plugin_command(binary, socket, &[]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();

        assert!(envs.contains_key(OsStr::new("ENE_PLUGIN_SOCKET")));
        assert!(envs.contains_key(OsStr::new("PATH")));
        assert!(!envs.contains_key(OsStr::new("ANTHROPIC_API_KEY")));
        assert!(!envs.contains_key(OsStr::new("AWS_SECRET_ACCESS_KEY")));
    }

    #[test]
    fn build_plugin_command_respects_passthrough() {
        use std::ffi::OsStr;

        let _guard = ENV_MUTEX.lock().unwrap();

        let binary = std::path::Path::new("/usr/bin/env");
        let socket = std::path::Path::new("/tmp/test.sock");

        unsafe { std::env::set_var("ENE_TEST_PASSTHROUGH_VAR", "hello") };
        let cmd = build_plugin_command(binary, socket, &["ENE_TEST_PASSTHROUGH_VAR".to_string()]);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        assert!(envs.contains_key(OsStr::new("ENE_TEST_PASSTHROUGH_VAR")));
        unsafe { std::env::remove_var("ENE_TEST_PASSTHROUGH_VAR") };

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

    // ── Per-plugin supervisor isolation ──────────────────────────────────

    /// A mock plugin that answers the handshake and replies to every `Ping`,
    /// incrementing `pings` on each probe. Accepts connections in a loop so a
    /// reconnect (if any) is also served. Used as the "healthy" plugin.
    #[cfg(unix)]
    async fn run_healthy_mock_server(
        socket_path: PathBuf,
        pings: Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::Ordering;

        use ene_plugin_proto::{
            IpcListener, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest,
            PluginIpcResponse, WireFormat, read_plugin_request, write_plugin_response,
        };

        ene_plugin_proto::cleanup_path(&socket_path);
        let Ok(mut listener) = IpcListener::bind(&socket_path) else {
            return;
        };
        loop {
            let Ok(stream) = listener.accept().await else {
                break;
            };
            let pings = Arc::clone(&pings);
            tokio::spawn(async move {
                let (mut read_half, write_half) = tokio::io::split(stream);
                let writer = Arc::new(Mutex::new(write_half));
                let mut format = WireFormat::Json;
                loop {
                    let Ok(Some(req)) = read_plugin_request(&mut read_half, format).await else {
                        break;
                    };
                    let resp_format = if matches!(&req, PluginIpcRequest::Handshake { .. }) {
                        format = WireFormat::for_version(PLUGIN_IPC_PROTOCOL_VERSION);
                        WireFormat::Json
                    } else {
                        format
                    };
                    let writer = Arc::clone(&writer);
                    let pings = Arc::clone(&pings);
                    tokio::spawn(async move {
                        let resp = match req {
                            PluginIpcRequest::Handshake { .. } => PluginIpcResponse::HandshakeAck {
                                version: PLUGIN_IPC_PROTOCOL_VERSION,
                                capabilities: PluginCapabilities {
                                    tools: 0,
                                    llm_providers: Vec::new(),
                                    tts_providers: Vec::new(),
                                    stt_providers: Vec::new(),
                                    ..PluginCapabilities::default()
                                },
                            },
                            PluginIpcRequest::Ping { request_id } => {
                                pings.fetch_add(1, Ordering::SeqCst);
                                PluginIpcResponse::Pong { request_id }
                            }
                            _ => PluginIpcResponse::Error {
                                request_id: String::new(),
                                message: "unsupported".to_string(),
                            },
                        };
                        let mut w = writer.lock().await;
                        drop(write_plugin_response(&mut *w, &resp, resp_format).await);
                    });
                }
            });
        }
    }

    /// A mock plugin that completes the handshake and then immediately closes
    /// the connection, so every subsequent `Ping` fails fast (EOF / broken
    /// pipe). Accepts connections in a loop so the host's reconnect attempts
    /// also handshake-then-close. Used as the "dead" plugin that drives its
    /// supervisor into the restart path.
    #[cfg(unix)]
    async fn run_dead_mock_server(socket_path: PathBuf) {
        use ene_plugin_proto::{
            IpcListener, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest,
            PluginIpcResponse, WireFormat, read_plugin_request, write_plugin_response,
        };

        ene_plugin_proto::cleanup_path(&socket_path);
        let Ok(mut listener) = IpcListener::bind(&socket_path) else {
            return;
        };
        loop {
            let Ok(stream) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (mut read_half, write_half) = tokio::io::split(stream);
                let writer = Mutex::new(write_half);
                // Answer only the handshake, then drop the stream so the host
                // observes a closed connection on its next request.
                if let Ok(Some(PluginIpcRequest::Handshake { .. })) =
                    read_plugin_request(&mut read_half, WireFormat::Json).await
                {
                    let mut w = writer.lock().await;
                    drop(
                        write_plugin_response(
                            &mut *w,
                            &PluginIpcResponse::HandshakeAck {
                                version: PLUGIN_IPC_PROTOCOL_VERSION,
                                capabilities: PluginCapabilities {
                                    tools: 0,
                                    llm_providers: Vec::new(),
                                    tts_providers: Vec::new(),
                                    stt_providers: Vec::new(),
                                    ..PluginCapabilities::default()
                                },
                            },
                            WireFormat::Json,
                        )
                        .await,
                    );
                }
                // Dropping the halves here closes the connection.
            });
        }
    }

    /// Builds a [`SupervisedPlugin`] whose `binary_path` is a genuine
    /// executable script (so a restart can re-spawn it) but whose
    /// `socket_path` points at `socket` — the path the supervisor pings and
    /// reconnects to. When `stay_alive` is true the child sleeps (so
    /// `is_alive()` reports the plugin as running); otherwise it exits
    /// immediately (so the supervisor sees a dead process and is driven into
    /// its restart path). Used by the supervisor-isolation tests, where the
    /// "plugin" is a mock IPC server rather than a real long-lived process.
    ///
    /// The dummy binary is written under `dir` (a per-test
    /// [`tempfile::tempdir`] that also holds the socket paths), so dropping
    /// the `TempDir` removes everything the test created — no pid-suffixed
    /// leftovers accumulate in the system temp dir.
    #[cfg(unix)]
    fn supervised_plugin_on_socket(
        dir: &std::path::Path,
        name: &str,
        socket: PathBuf,
        stay_alive: bool,
    ) -> SupervisedPlugin {
        let binary_path = dir.join(format!("ene-plugin-{name}"));
        write_dummy_script(&binary_path);
        let real = compute_binary_checksum(name, &binary_path).unwrap();

        let child = if stay_alive {
            std::process::Command::new("sh")
                .args(["-c", "sleep 30"])
                .spawn()
                .unwrap()
        } else {
            std::process::Command::new("sh")
                .args(["-c", "exit 0"])
                .spawn()
                .unwrap()
        };

        SupervisedPlugin {
            name: name.to_string(),
            child,
            socket_path: socket,
            binary_path,
            pinned_checksum: PinnedChecksum::Tofu(real),
            env_passthrough: Vec::new(),
            restart_times: VecDeque::new(),
            disabled: false,
        }
    }

    /// [`spawn_supervisors`] (the wiring used by `PluginHostManager::start`)
    /// must return exactly one supervisor handle per (supervised, connection)
    /// pair. A spawn-wiring bug that starts fewer tasks than plugins would
    /// make this assertion fail; a `supervise_plugin`-level test cannot catch
    /// that because it always drives the function with already-paired
    /// arguments.
    #[tokio::test]
    #[cfg(unix)]
    async fn spawn_supervisors_returns_one_handle_per_plugin() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (tx, _rx) = mpsc::unbounded_channel::<PluginHealthEvent>();
        assert!(spawn_supervisors(Duration::ZERO, &[], &[], &tx, |_count| async {}).is_empty());
        assert!(
            spawn_supervisors(Duration::from_millis(30), &[], &[], &tx, |_count| async {},)
                .is_empty()
        );

        let temp = tempfile::tempdir().expect("OS allows temp directory creation");
        let pings_a = Arc::new(AtomicUsize::new(0));
        let pings_b = Arc::new(AtomicUsize::new(0));
        let sock_a = temp.path().join("wiring-a.sock");
        let sock_b = temp.path().join("wiring-b.sock");
        let server_a = tokio::spawn(run_healthy_mock_server(
            sock_a.clone(),
            Arc::clone(&pings_a),
        ));
        let server_b = tokio::spawn(run_healthy_mock_server(
            sock_b.clone(),
            Arc::clone(&pings_b),
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let supervised = vec![
            Arc::new(Mutex::new(supervised_plugin_on_socket(
                temp.path(),
                "wiring-a",
                sock_a.clone(),
                true,
            ))),
            Arc::new(Mutex::new(supervised_plugin_on_socket(
                temp.path(),
                "wiring-b",
                sock_b.clone(),
                true,
            ))),
        ];
        let connections = vec![
            Arc::new(
                IpcPluginConnection::connect(
                    &sock_a,
                    ene_plugin_proto::SandboxConfigData::default(),
                    None,
                    None,
                    Duration::from_secs(5),
                    8,
                )
                .await
                .expect("wiring-a handshake"),
            ),
            Arc::new(
                IpcPluginConnection::connect(
                    &sock_b,
                    ene_plugin_proto::SandboxConfigData::default(),
                    None,
                    None,
                    Duration::from_secs(5),
                    8,
                )
                .await
                .expect("wiring-b handshake"),
            ),
        ];

        let handles = spawn_supervisors(
            Duration::from_millis(30),
            &supervised,
            &connections,
            &tx,
            |_count| async {},
        );
        assert_eq!(
            handles.len(),
            2,
            "start() must spawn one supervisor task per (supervised, connection) pair"
        );

        // Each spawned supervisor really monitors its own plugin: both mock
        // peers get probed on their own schedule.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if pings_a.load(Ordering::SeqCst) >= 1 && pings_b.load(Ordering::SeqCst) >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "spawned supervisors did not ping their own plugins in time"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        for handle in handles {
            handle.abort();
        }
        server_a.abort();
        server_b.abort();
    }

    /// One plugin's restart backoff must not stop another plugin from being
    /// monitored.
    ///
    /// Two plugins are supervised by independent tasks: `healthy` answers
    /// pings (its probe count is observed server-side), `dead` closes every
    /// connection so each probe fails and drives its supervisor into the
    /// restart path. The restart backoff is injected as a gate the test
    /// controls. While `dead` is parked in that backoff, `healthy` must keep
    /// being pinged — with a single shared loop the probe would be stuck
    /// behind `dead`'s backoff and `healthy`'s ping count would not advance.
    #[tokio::test]
    #[cfg(unix)]
    async fn one_plugin_backoff_does_not_block_another_supervisor() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // One temp dir per test: the socket paths and the dummy plugin
        // binaries all live under it, and dropping the `TempDir` removes them
        // (no pid-suffixed leftovers accumulate in the system temp dir).
        let temp = tempfile::tempdir().expect("OS allows temp directory creation");
        let healthy_sock = temp.path().join("healthy.sock");
        let dead_sock = temp.path().join("dead.sock");

        let healthy_pings = Arc::new(AtomicUsize::new(0));
        let healthy_server = tokio::spawn(run_healthy_mock_server(
            healthy_sock.clone(),
            Arc::clone(&healthy_pings),
        ));
        let dead_server = tokio::spawn(run_dead_mock_server(dead_sock.clone()));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let healthy = Arc::new(Mutex::new(supervised_plugin_on_socket(
            temp.path(),
            "healthy",
            healthy_sock.clone(),
            true,
        )));
        let dead = Arc::new(Mutex::new(supervised_plugin_on_socket(
            temp.path(),
            "dead",
            dead_sock.clone(),
            false,
        )));

        let healthy_conn = Arc::new(
            IpcPluginConnection::connect(
                &healthy_sock,
                ene_plugin_proto::SandboxConfigData::default(),
                None,
                None,
                Duration::from_secs(5),
                8,
            )
            .await
            .expect("healthy handshake"),
        );
        let dead_conn = Arc::new(
            IpcPluginConnection::connect(
                &dead_sock,
                ene_plugin_proto::SandboxConfigData::default(),
                None,
                None,
                Duration::from_secs(5),
                8,
            )
            .await
            .expect("dead handshake"),
        );

        let (tx, mut rx) = mpsc::unbounded_channel::<PluginHealthEvent>();
        let interval = Duration::from_millis(30);

        // Healthy supervisor: never expected to hit its (no-op) backoff.
        let healthy_task = tokio::spawn(supervise_plugin(
            interval,
            Arc::clone(&healthy),
            Arc::clone(&healthy_conn),
            tx.clone(),
            |_count| async {},
        ));

        // Dead supervisor: its backoff parks on the gate until the test
        // releases it, standing in for the (up to 30 s) real backoff.
        let dead_backoffs = Arc::new(AtomicUsize::new(0));
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
        let gate_rx = Arc::new(Mutex::new(Some(gate_rx)));
        let db = Arc::clone(&dead_backoffs);
        let gate = Arc::clone(&gate_rx);
        let dead_task = tokio::spawn(supervise_plugin(
            interval,
            Arc::clone(&dead),
            Arc::clone(&dead_conn),
            tx,
            move |_count| {
                let db = Arc::clone(&db);
                let gate = Arc::clone(&gate);
                async move {
                    db.fetch_add(1, Ordering::SeqCst);
                    if let Some(rx) = gate.lock().await.take() {
                        drop(rx.await);
                    }
                }
            },
        ));

        // Wait until the dead supervisor has reached its (parked) backoff and
        // the healthy supervisor has pinged at least twice.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if dead_backoffs.load(Ordering::SeqCst) >= 1
                && healthy_pings.load(Ordering::SeqCst) >= 2
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "supervisors did not reach the expected state in time"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // While the dead plugin is parked in its backoff, the healthy plugin
        // must keep being pinged. With a single shared loop the probe would
        // be stuck behind the dead plugin's backoff and this count would not
        // advance.
        let before = healthy_pings.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after = healthy_pings.load(Ordering::SeqCst);
        assert!(
            after > before,
            "healthy plugin monitoring stalled while another plugin was in \
             restart backoff (before={before}, after={after})"
        );

        // The dead plugin was flagged unhealthy on its way into the backoff.
        let mut saw_unhealthy = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, PluginHealthEvent::Unhealthy { .. }) {
                saw_unhealthy = true;
            }
        }
        assert!(
            saw_unhealthy,
            "dead plugin should have been flagged unhealthy"
        );

        // Releasing the backoff lets the dead supervisor proceed to a restart,
        // confirming the park — not a hang — was what held it.
        drop(gate_tx);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if dead.lock().await.recent_restart_count() >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "dead plugin did not restart after its backoff was released"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        healthy_task.abort();
        dead_task.abort();
        healthy_server.abort();
        dead_server.abort();
        // The socket files and dummy binaries live under `temp` and are
        // removed when the `TempDir` drops at the end of the test.
    }

    /// A healthy probe round-trip must not clear the rolling restart window.
    /// Only elapsed time prunes entries — otherwise a crash loop that answers
    /// one ping between restarts would never hit the budget.
    #[tokio::test]
    #[cfg(unix)]
    async fn healthy_probe_does_not_clear_restart_window() {
        use std::sync::atomic::AtomicUsize;

        let temp = tempfile::tempdir().expect("OS allows temp directory creation");
        let sock = temp.path().join("recover.sock");
        let pings = Arc::new(AtomicUsize::new(0));
        let server = tokio::spawn(run_healthy_mock_server(sock.clone(), Arc::clone(&pings)));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let plugin = Arc::new(Mutex::new(supervised_plugin_on_socket(
            temp.path(),
            "recover",
            sock.clone(),
            true, // stays alive so the probe sees a healthy round-trip
        )));
        // Simulate a full window of recent restarts.
        {
            let mut p = plugin.lock().await;
            let now = Instant::now();
            for _ in 0..MAX_RESTARTS {
                p.restart_times.push_back(now);
            }
        }

        let conn = Arc::new(
            IpcPluginConnection::connect(
                &sock,
                ene_plugin_proto::SandboxConfigData::default(),
                None,
                None,
                Duration::from_secs(5),
                8,
            )
            .await
            .expect("recover handshake"),
        );

        let (tx, _rx) = mpsc::unbounded_channel::<PluginHealthEvent>();
        let task = tokio::spawn(supervise_plugin(
            Duration::from_millis(30),
            Arc::clone(&plugin),
            Arc::clone(&conn),
            tx,
            |_count| async {},
        ));

        // Wait until at least one healthy probe has run.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if pings.load(std::sync::atomic::Ordering::SeqCst) >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "healthy probe did not run in time"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Give the supervisor a moment to process the healthy path.
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(
            plugin.lock().await.recent_restart_count(),
            MAX_RESTARTS,
            "a healthy probe must not clear the rolling restart window"
        );
        assert!(
            !plugin.lock().await.disabled,
            "a healthy plugin must not be disabled"
        );

        task.abort();
        server.abort();
    }

    /// A dead plugin whose rolling window is already full is disabled (not
    /// restarted) — crash-loop detection is preserved.
    #[tokio::test]
    #[cfg(unix)]
    async fn exhausted_budget_disables_dead_plugin() {
        let temp = tempfile::tempdir().expect("OS allows temp directory creation");
        let sock = temp.path().join("exhaust.sock");
        let server = tokio::spawn(run_dead_mock_server(sock.clone()));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let plugin = Arc::new(Mutex::new(supervised_plugin_on_socket(
            temp.path(),
            "exhaust",
            sock.clone(),
            false, // exits immediately so the probe sees a dead process
        )));
        // Window already consumed by prior crash-loop restarts.
        {
            let mut p = plugin.lock().await;
            let now = Instant::now();
            for _ in 0..MAX_RESTARTS {
                p.restart_times.push_back(now);
            }
        }

        let conn = Arc::new(
            IpcPluginConnection::connect(
                &sock,
                ene_plugin_proto::SandboxConfigData::default(),
                None,
                None,
                Duration::from_secs(5),
                8,
            )
            .await
            .expect("exhaust handshake"),
        );

        let (tx, mut rx) = mpsc::unbounded_channel::<PluginHealthEvent>();
        let task = tokio::spawn(supervise_plugin(
            Duration::from_millis(30),
            Arc::clone(&plugin),
            Arc::clone(&conn),
            tx,
            |_count| async {},
        ));

        // The supervisor must observe the dead plugin, see the exhausted
        // budget inside `restart()`, and disable it rather than restarting.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if plugin.lock().await.disabled {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "an exhausted budget on a dead plugin must disable it"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let mut saw_disabled = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(
                event,
                PluginHealthEvent::Disabled {
                    reason: crate::health::DisabledReason::RestartBudgetExhausted,
                    ..
                }
            ) {
                saw_disabled = true;
            }
        }
        assert!(
            saw_disabled,
            "expected a RestartBudgetExhausted Disabled event"
        );

        task.abort();
        server.abort();
    }

    /// Kinds-only stub factories, so eviction tests can distinguish entries.
    struct KindLlmFactory(&'static str);

    impl ene_ai::LlmProviderFactory for KindLlmFactory {
        fn provider_name(&self) -> &str {
            self.0
        }

        fn create_provider(
            &self,
            _config: &ene_config::EneConfig,
            _task: &ene_ai::TaskRef,
        ) -> Result<Box<dyn ene_ai::LlmProvider>, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider("stub".to_string()))
        }
    }

    struct KindEmbeddingFactory(&'static str);

    impl ene_ai::EmbeddingProviderFactory for KindEmbeddingFactory {
        fn provider_kind(&self) -> &str {
            self.0
        }

        fn create_embedding_provider(
            &self,
            _config: &ene_config::EneConfig,
        ) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
            Err(ene_ai::EmbeddingError::Init("stub".to_string()))
        }
    }

    struct KindTtsFactory(&'static str);

    impl ene_ai::TtsProviderFactory for KindTtsFactory {
        fn provider_name(&self) -> &str {
            self.0
        }

        fn create_provider(
            &self,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn ene_ai::TtsProvider>, ene_ai::AudioProviderError> {
            Err(ene_ai::AudioProviderError::Provider("stub".to_string()))
        }
    }

    #[test]
    fn remove_provider_factories_if_match_evicts_same_generation_handles() {
        let mut manager = PluginHostManager::test_instance();
        // The handles a health bridge would capture: identical Arcs to the
        // factories the manager stores.
        let handles = plugin_factory_handles();
        manager
            .llm_factories
            .insert("openai".to_string(), Arc::clone(&handles.llm[0].1));
        manager
            .llm_factory_plugins
            .insert("openai".to_string(), "openai-plugin".to_string());
        // A different plugin's kind must survive the eviction untouched.
        manager.llm_factories.insert(
            "anthropic".to_string(),
            Arc::new(KindLlmFactory("anthropic")) as Arc<dyn ene_ai::LlmProviderFactory>,
        );
        manager
            .llm_factory_plugins
            .insert("anthropic".to_string(), "anthropic-plugin".to_string());
        manager
            .embedding_factories
            .insert("openai".to_string(), Arc::clone(&handles.embedding[0].1));
        manager
            .embedding_factory_plugins
            .insert("openai".to_string(), "openai-plugin".to_string());
        manager
            .tts_factories
            .insert("kokoro".to_string(), Arc::clone(&handles.tts[0].1));
        manager
            .tts_factory_plugins
            .insert("kokoro".to_string(), "openai-plugin".to_string());

        let removal = manager.remove_provider_factories_if_match(&handles);
        assert_eq!(
            removal,
            ProviderFactoryRemoval {
                llm: 1,
                embedding: 1,
                tts: 1,
            }
        );
        assert!(!manager.llm_factories.contains_key("openai"));
        assert!(manager.llm_factories.contains_key("anthropic"));
        assert!(!manager.embedding_factories.contains_key("openai"));
        assert!(!manager.tts_factories.contains_key("kokoro"));
        assert!(!manager.llm_factory_plugins.contains_key("openai"));

        assert!(
            manager
                .remove_provider_factories_if_match(&handles)
                .is_empty()
        );
    }

    #[test]
    fn remove_provider_factories_if_match_ignores_stale_generation_handles() {
        let mut manager = PluginHostManager::test_instance();
        // The current host serves the kind with a fresh factory; the stale
        // handles come from the host generation that emitted a Disabled
        // event before a reconfiguration swapped this host in.
        manager.llm_factories.insert(
            "openai".to_string(),
            Arc::new(KindLlmFactory("openai")) as Arc<dyn ene_ai::LlmProviderFactory>,
        );
        manager
            .llm_factory_plugins
            .insert("openai".to_string(), "openai-plugin".to_string());

        let stale = PluginFactoryHandles {
            llm: vec![(
                "openai".to_string(),
                Arc::new(KindLlmFactory("openai")) as LlmFactoryHandle,
            )],
            ..PluginFactoryHandles::default()
        };
        let removal = manager.remove_provider_factories_if_match(&stale);
        assert_eq!(
            removal,
            ProviderFactoryRemoval {
                llm: 0,
                embedding: 0,
                tts: 0,
            }
        );
        assert!(
            manager.llm_factories.contains_key("openai"),
            "a stale event must not evict the replacement host's factory"
        );
    }

    fn plugin_factory_handles() -> PluginFactoryHandles {
        PluginFactoryHandles {
            llm: vec![(
                "openai".to_string(),
                Arc::new(KindLlmFactory("openai")) as LlmFactoryHandle,
            )],
            embedding: vec![(
                "openai".to_string(),
                Arc::new(KindEmbeddingFactory("openai")) as EmbeddingFactoryHandle,
            )],
            tts: vec![(
                "kokoro".to_string(),
                Arc::new(KindTtsFactory("kokoro")) as TtsFactoryHandle,
            )],
        }
    }
}
