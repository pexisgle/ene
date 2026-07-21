use crate::circuit_breaker::CircuitBreaker;
use crate::error::ToolHostError;
use crate::health::ToolHealthEvent;
use crate::ipc_registry::IpcToolRegistry;
use crate::tools::CompositeToolRegistry;
use crate::tools::registry::ToolRegistry;
use ene_config as paths;
use ene_config::{EneConfig, register_runtime_schema};
use ene_tool_proto::ToolError;
use ene_tool_proto::ToolSpec;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

const MAX_RESTARTS: usize = 5;
const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 30_000;
/// Maximum number of connection retries when connecting to a tool binary.
pub(crate) const CONNECT_RETRIES: u32 = 50;
/// Delay in milliseconds between connection retry attempts.
pub(crate) const CONNECT_DELAY_MS: u64 = 50;

struct ToolProcess {
    name: String,
    child: std::process::Child,
    socket_path: PathBuf,
    binary_path: PathBuf,
    sandbox: ene_tool_proto::SandboxConfigData,
    tool_config: Option<serde_json::Value>,
    restart_count: usize,
}

impl ToolProcess {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn restart(&mut self) -> Result<(), ToolHostError> {
        self.restart_count = self.restart_count.saturating_add(1);
        if self.restart_count > MAX_RESTARTS {
            return Err(ToolHostError::ExecutionFailed {
                message: format!(
                    "Tool '{}' exceeded max restarts ({})",
                    self.name, MAX_RESTARTS
                ),
            });
        }

        let _ = self.child.kill();
        let _ = self.child.wait();

        ene_tool_proto::transport::cleanup_path(&self.socket_path);

        tracing::warn!(
            "[ToolHostManager] Restarting tool '{}' (attempt {}/{})",
            self.name,
            self.restart_count,
            MAX_RESTARTS
        );

        let child = std::process::Command::new(&self.binary_path)
            .env("ENE_TOOL_SOCKET", &self.socket_path)
            .spawn()
            .map_err(|e| ToolHostError::ExecutionFailed {
                message: format!("Failed to restart '{}': {}", self.binary_path.display(), e),
            })?;

        self.child = child;
        Ok(())
    }

    fn delay_for_restart(&self) -> Duration {
        delay_for_restart(self.restart_count)
    }
}

/// Pure-function form of the per-restart backoff delay.
/// Exposed at module scope so unit tests can verify the
/// schedule without having to construct a `ToolProcess`
/// (which owns an unconstructible-by-mem-zero `Child`).
fn delay_for_restart(restart_count: usize) -> Duration {
    let delay_ms = BASE_DELAY_MS.saturating_mul(2u64.saturating_pow(restart_count as u32));
    Duration::from_millis(delay_ms.min(MAX_DELAY_MS))
}

impl Drop for ToolProcess {
    fn drop(&mut self) {
        tracing::info!(component = "ToolHostManager", tool = %self.name, "Stopping tool");
        let _ = self.child.kill();
        ene_tool_proto::transport::cleanup_path(&self.socket_path);
    }
}

struct SupervisedIpcRegistry {
    process: Arc<Mutex<ToolProcess>>,
    registry: std::sync::RwLock<Arc<IpcToolRegistry>>,
    /// Per-call timeout, forwarded to `IpcToolRegistry::new` on
    /// every (re)connect.
    timeout_ms: u64,
    /// Per-tool circuit breaker guarding consecutive call failures (#238).
    breaker: Mutex<CircuitBreaker>,
    /// Sink for health events surfaced to the diagnostics layer (#238).
    health_tx: Option<mpsc::UnboundedSender<ToolHealthEvent>>,
}

impl SupervisedIpcRegistry {
    fn emit_health(&self, event: ToolHealthEvent) {
        if let Some(tx) = &self.health_tx {
            let _ = tx.send(event);
        }
    }

    /// Restarts the tool process, reconnects, and installs the fresh
    /// registry. Assumes the caller has already verified a restart is
    /// warranted. Emits `Restarting`/`Restarted`/`Disabled` health events.
    async fn restart_and_reconnect(&self) -> Result<Arc<IpcToolRegistry>, ToolHostError> {
        let mut guard = self.process.lock().await;

        if guard.restart_count >= MAX_RESTARTS {
            self.emit_health(ToolHealthEvent::Disabled {
                tool: guard.name.clone(),
            });
            return Err(ToolHostError::ExecutionFailed {
                message: format!(
                    "Tool '{}' has exceeded max restarts ({}) and is disabled",
                    guard.name, MAX_RESTARTS
                ),
            });
        }

        let attempt = guard.restart_count.saturating_add(1);
        self.emit_health(ToolHealthEvent::Restarting {
            tool: guard.name.clone(),
            attempt,
        });

        let delay = guard.delay_for_restart();
        tokio::time::sleep(delay).await;

        guard
            .restart()
            .map_err(|e| ToolHostError::ExecutionFailed {
                message: e.to_string(),
            })?;

        let socket_path = guard.socket_path.clone();
        let sandbox = guard.sandbox.clone();
        let tool_config = guard.tool_config.clone();
        let name = guard.name.clone();
        drop(guard);

        let new_registry = match ToolHostManager::connect_with_retry(
            &socket_path,
            &sandbox,
            tool_config,
            CONNECT_RETRIES,
            CONNECT_DELAY_MS,
            self.timeout_ms,
        )
        .await
        {
            Ok(reg) => reg,
            Err(e) => {
                let mut guard = self.process.lock().await;
                let _ = guard.child.kill();
                let _ = guard.child.wait();
                return Err(ToolHostError::ExecutionFailed {
                    message: format!("Failed to connect to restarted tool '{name}': {e}"),
                });
            }
        };

        let new_reg_arc = Arc::new(new_registry);
        {
            let mut reg_guard = self
                .registry
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *reg_guard = Arc::clone(&new_reg_arc);
        }
        self.emit_health(ToolHealthEvent::Restarted { tool: name });

        Ok(new_reg_arc)
    }

    /// Periodic liveness probe (#238). Pings the tool and, when it is dead
    /// or unresponsive, restarts it. Returns `true` when the tool is healthy
    /// (or was successfully healed).
    async fn probe_and_heal(&self) -> bool {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let ping_ok = reg.ping().await.is_ok();
        let alive = self.process.lock().await.is_alive();

        if alive && ping_ok {
            return true;
        }

        let reason = if alive { "unresponsive" } else { "dead" };
        let tool = self.process.lock().await.name.clone();
        self.emit_health(ToolHealthEvent::Unhealthy {
            tool: tool.clone(),
            reason: reason.to_string(),
        });
        tracing::warn!(
            "[SupervisedIpcRegistry] Health probe: tool '{}' is {} (alive={alive}, ping={ping_ok}); restarting",
            tool,
            reason
        );

        match self.restart_and_reconnect().await {
            Ok(_) => {
                self.emit_health(ToolHealthEvent::Recovered { tool });
                true
            }
            Err(e) => {
                tracing::error!(
                    "[SupervisedIpcRegistry] Health probe: failed to heal tool '{}': {e}",
                    tool
                );
                false
            }
        }
    }
}

/// Background loop that periodically pings every supervised tool and
/// restarts any that are dead or unresponsive (#238).
#[expect(
    clippy::infinite_loop,
    reason = "background liveness probe runs for the lifetime of the tool host"
)]
async fn health_probe_loop(interval: Duration, registries: Vec<Arc<SupervisedIpcRegistry>>) {
    let mut ticker = tokio::time::interval(interval);
    // Skip the immediate first tick so we don't probe right after startup.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        for registry in &registries {
            let _ = registry.probe_and_heal().await;
        }
    }
}

#[async_trait::async_trait]
impl ToolRegistry for SupervisedIpcRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.list_tools()
    }

    fn list_rag_profiles(&self) -> Vec<ene_tool_proto::ToolRagProfile> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.list_rag_profiles()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
        // Circuit breaker: reject fast while the tool is paused after
        // repeated consecutive failures (#238).
        {
            let mut breaker = self.breaker.lock().await;
            if breaker.is_open() {
                return Err(ToolHostError::CircuitOpen {
                    tool: name.to_string(),
                    consecutive_failures: breaker.consecutive_failures(),
                });
            }
        }

        // Fast path: a successful call also clears the
        // per-process restart counter so a tool that crashes
        // occasionally but is otherwise healthy does not
        // accumulate lifetime crashes against the MAX_RESTARTS
        // budget.
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let first_result = reg.call_tool(name, arguments).await;
        if first_result.is_ok() {
            {
                let mut breaker = self.breaker.lock().await;
                if breaker.consecutive_failures() != 0 {
                    self.emit_health(ToolHealthEvent::CircuitClosed {
                        tool: name.to_string(),
                    });
                }
                breaker.record_success();
            }
            let mut guard = self.process.lock().await;
            if guard.restart_count != 0 {
                tracing::info!(
                    "[SupervisedIpcRegistry] Tool '{}' call succeeded; resetting restart counter (was {})",
                    guard.name,
                    guard.restart_count
                );
                guard.restart_count = 0;
            }
            drop(guard);
            return first_result;
        }

        // Record the failure against the circuit breaker; if it trips,
        // surface the original error and pause subsequent calls (#238).
        {
            let mut breaker = self.breaker.lock().await;
            if breaker.record_failure() {
                let failures = breaker.consecutive_failures();
                self.emit_health(ToolHealthEvent::CircuitOpened {
                    tool: name.to_string(),
                    consecutive_failures: failures,
                });
                tracing::warn!(
                    "[SupervisedIpcRegistry] Circuit breaker opened for tool '{}' after {} consecutive failures",
                    name,
                    failures
                );
            }
        }

        // Supervision path: the call failed. Distinguish a tool-owned
        // error (process alive and responsive) from a dead or hung
        // process. A hung process is alive at the OS level but fails a
        // short IPC `Ping` probe; both dead and hung tools are restarted
        // (#238).
        let probe = reg.ping().await;
        let mut guard = self.process.lock().await;

        if guard.is_alive() && probe.is_ok() {
            // Process is alive and responsive; the failure was the
            // tool's own response (e.g. a permission denial).
            // Surface the original error to the caller.
            return first_result;
        }

        let reason = if guard.is_alive() {
            "unresponsive"
        } else {
            "dead"
        };
        let alive = guard.is_alive();
        let tool_name = guard.name.clone();
        self.emit_health(ToolHealthEvent::Unhealthy {
            tool: tool_name.clone(),
            reason: reason.to_string(),
        });
        tracing::warn!(
            "[SupervisedIpcRegistry] Tool '{}' is {} (alive={}, ping={:?}), attempting restart",
            tool_name,
            reason,
            alive,
            probe.is_ok()
        );
        drop(guard);

        let new_reg_arc = self.restart_and_reconnect().await?;

        // Retry on the freshly installed registry. The process
        // lock is not held across connect/retry; concurrent
        // callers that arrive while the child is already alive
        // after restart surface their original error without a
        // second restart.
        new_reg_arc.call_tool(name, arguments).await
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<crate::DeferredCallResult, ToolHostError> {
        // Deferred acceptance is a fast, non-blocking round-trip: the
        // tool either returns a `task_id` immediately or falls back to a
        // synchronous result. It does not run the long-lived work inline,
        // so the supervised restart path used by `call_tool` is not needed
        // here (#196).
        {
            let mut breaker = self.breaker.lock().await;
            if breaker.is_open() {
                return Err(ToolHostError::CircuitOpen {
                    tool: name.to_string(),
                    consecutive_failures: breaker.consecutive_failures(),
                });
            }
        }
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.call_tool_deferred(name, arguments).await
    }

    async fn poll_deferred(
        &self,
        tool_name: &str,
        task_id: &str,
    ) -> ene_tool_proto::DeferredStatus {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.poll_deferred(tool_name, task_id).await
    }

    async fn cancel_deferred(&self, tool_name: &str, task_id: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.cancel_deferred(tool_name, task_id).await;
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.config_schema().await
    }

    async fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.set_call_context(ctx).await;
    }

    async fn approve_permission(&self, request_id: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.approve_permission(request_id).await;
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.allow_pattern(action, target_pattern).await;
    }

    async fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        reg.revoke_pattern(action, target_pattern).await;
    }
}

/// Orchestrates the lifecycle of all tool processes.
///
/// Reads the `tools` section from settings, discovers and spawns tool binaries,
/// wraps each in a `SupervisedIpcRegistry` with crash detection and auto-restart,
/// and aggregates them into a [`CompositeToolRegistry`].
///
/// Also supports adding external registries (e.g., [`crate::McpToolRegistry`]).
/// Tool RAG indexing and selection is handled by [`ToolRag`](crate::ToolRag).
pub struct ToolHostManager {
    composite: Arc<CompositeToolRegistry>,
    /// Receiver for health events emitted by the supervisor and the
    /// periodic probe; bridged into runtime diagnostics (#238).
    health_rx: Option<mpsc::UnboundedReceiver<ToolHealthEvent>>,
    /// Handle to the periodic health-probe task so it is aborted on drop.
    _health_task: Option<tokio::task::JoinHandle<()>>,
}

#[async_trait::async_trait]
impl ToolRegistry for ToolHostManager {
    fn list_tools(&self) -> Vec<ToolSpec> {
        self.composite.list_tools()
    }

    fn list_rag_profiles(&self) -> Vec<ene_tool_proto::ToolRagProfile> {
        self.composite.list_rag_profiles()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
        self.composite.call_tool(name, arguments).await
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<crate::DeferredCallResult, ToolHostError> {
        self.composite.call_tool_deferred(name, arguments).await
    }

    async fn poll_deferred(
        &self,
        tool_name: &str,
        task_id: &str,
    ) -> ene_tool_proto::DeferredStatus {
        self.composite.poll_deferred(tool_name, task_id).await
    }

    async fn cancel_deferred(&self, tool_name: &str, task_id: &str) {
        self.composite.cancel_deferred(tool_name, task_id).await;
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        self.composite.config_schema().await
    }

    async fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        self.composite.set_call_context(ctx).await;
    }

    async fn approve_permission(&self, request_id: &str) {
        self.composite.approve_permission(request_id).await;
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.composite.allow_pattern(action, target_pattern).await;
    }

    async fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.composite.revoke_pattern(action, target_pattern).await;
    }
}

impl ToolHostManager {
    /// Starts all enabled tool binaries from the settings configuration.
    ///
    /// Creates the socket directory, iterates over enabled tools in `settings.tools`,
    /// spawns each binary as a child process, and connects to it over IPC.
    /// Also registers each tool's config schema in the global runtime registry
    /// and regenerates `settings.schema.json`.
    pub async fn start(
        config: &EneConfig,
        mut db_tokens: std::collections::HashMap<String, String>,
    ) -> Result<Self, ToolHostError> {
        let tool_config = config
            .get_section::<crate::config::ToolConfig>()
            .unwrap_or_default();

        let mut supervised: Vec<Arc<SupervisedIpcRegistry>> = Vec::new();

        std::fs::create_dir_all(paths::tool_socket_dir()).map_err(|e| {
            ToolHostError::ExecutionFailed {
                message: format!("Failed to create socket dir: {e}"),
            }
        })?;

        let (health_tx, health_rx) = mpsc::unbounded_channel::<ToolHealthEvent>();

        let timeout_ms = tool_config.timeout_ms;
        let breaker = CircuitBreaker::new(
            tool_config.circuit_failure_threshold,
            Duration::from_millis(tool_config.circuit_cooldown_ms),
        );
        for (name, entry) in &tool_config.list {
            if !entry.enable {
                continue;
            }
            let tool_config = match &entry.config {
                serde_json::Value::Object(m) if m.is_empty() => None,
                _ => Some(entry.config.clone()),
            };
            let mut sandbox =
                serde_json::from_value::<ene_tool_proto::SandboxConfigData>(entry.config.clone())
                    .unwrap_or_default();
            sandbox.sanitize();
            let db_token = db_tokens.remove(name);
            match Self::start_tool(
                name,
                &sandbox,
                tool_config,
                timeout_ms,
                db_token,
                breaker.clone(),
                health_tx.clone(),
            )
            .await
            {
                Ok(supervised_entry) => {
                    if let Some(schema) = supervised_entry.config_schema().await {
                        let schema_key = format!("{name}_config");
                        register_runtime_schema(&schema_key, schema);
                    }
                    supervised.push(supervised_entry);
                }
                Err(e) => {
                    tracing::error!(component = "ToolHostManager", tool = %name, error = %e, "Failed to start tool");
                }
            }
        }

        let composite_registries: Vec<Arc<dyn ToolRegistry>> = supervised
            .iter()
            .map(|r| Arc::clone(r) as Arc<dyn ToolRegistry>)
            .collect();
        let composite = Arc::new(CompositeToolRegistry::try_new(composite_registries)?);

        // Spawn the periodic liveness probe (#238). It pings each tool on a
        // fixed interval and restarts any that are dead or unresponsive.
        let health_task = if tool_config.health_interval_ms > 0 && !supervised.is_empty() {
            let interval = Duration::from_millis(tool_config.health_interval_ms);
            let probes: Vec<Arc<SupervisedIpcRegistry>> =
                supervised.iter().map(Arc::clone).collect();
            Some(tokio::spawn(async move {
                health_probe_loop(interval, probes).await;
            }))
        } else {
            None
        };

        Ok(Self {
            composite,
            health_rx: Some(health_rx),
            _health_task: health_task,
        })
    }

    /// Takes ownership of the health-event receiver (#238).
    ///
    /// The runtime calls this once after startup to bridge tool health
    /// events into its diagnostics channel. Returns `None` on subsequent
    /// calls.
    pub fn take_health_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<ToolHealthEvent>> {
        self.health_rx.take()
    }

    /// Starts all tool binaries and MCP servers and returns the unified registry.
    ///
    /// Combines [`start`](Self::start) (IPC tool spawn), MCP server connection,
    /// and registry aggregation into a single call. Includes automatic fallback
    /// to an empty tool set if the primary startup fails.
    ///
    /// When `health_tx` is provided, tool health events (#238) are forwarded
    /// to it for the lifetime of the returned registry.
    pub async fn start_full(
        config: &EneConfig,
        db_tokens: std::collections::HashMap<String, String>,
        health_tx: Option<mpsc::UnboundedSender<ToolHealthEvent>>,
    ) -> Result<Arc<dyn ToolRegistry>, ToolHostError> {
        let mut manager = match Self::start(config, db_tokens).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "[ToolHostManager] Failed to start tool host, falling back to empty tools: {}",
                    e
                );
                let mut fallback_config = config.clone();
                let fallback_tools = crate::config::ToolConfig {
                    list: std::collections::HashMap::new(),
                    ..Default::default()
                };
                let _ = fallback_config.set_section(&fallback_tools);
                Self::start(&fallback_config, std::collections::HashMap::new())
                    .await
                    .map_err(|e2| ToolHostError::ExecutionFailed {
                        message: format!("Fatal: Failed to start fallback ToolHostManager: {e2}"),
                    })?
            }
        };

        let mcp_servers = config
            .get_section::<crate::config::ToolConfig>()
            .map(|tc| tc.mcp_servers)
            .unwrap_or_default();
        if !mcp_servers.is_empty() {
            let mcp = crate::McpToolRegistry::new();
            for server in &mcp_servers {
                if !server.enabled {
                    continue;
                }
                match &server.transport {
                    crate::mcp_config::McpTransport::Stdio { command, args } => {
                        let args_ref: Vec<&str> =
                            args.iter().map(std::string::String::as_str).collect();
                        if let Err(err) = mcp.connect_stdio(&server.name, command, &args_ref).await
                        {
                            tracing::warn!(
                                "MCP server '{}' failed to connect: {}",
                                server.name,
                                err
                            );
                        }
                    }
                    crate::mcp_config::McpTransport::Http { url } => {
                        tracing::warn!(
                            "MCP HTTP transport not supported yet for '{}' (URL: {})",
                            server.name,
                            url
                        );
                    }
                }
            }
            manager.try_add_registry(Arc::new(mcp))?;
        }

        // Bridge tool health events to the runtime diagnostics layer (#238).
        if let (Some(sink), Some(mut rx)) = (health_tx, manager.take_health_receiver()) {
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if sink.send(event).is_err() {
                        break;
                    }
                }
            });
        }

        Ok(manager.into_registry())
    }

    /// Add a manual registry to the manager. Useful for testing or injecting custom MCP registries.
    ///
    /// # Errors
    /// Propagates [`ToolHostError::DuplicateToolName`] when the registry
    /// collides with an already-indexed tool name.
    pub fn try_add_registry(
        &mut self,
        registry: Arc<dyn ToolRegistry>,
    ) -> Result<(), ToolHostError> {
        self.composite.try_add_registry(registry)
    }

    /// Consume the manager and return a unified [`CompositeToolRegistry`] containing all added registries.
    pub fn into_registry(self) -> Arc<dyn ToolRegistry> {
        Arc::new(self)
    }

    async fn start_tool(
        name: &str,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
        timeout_ms: u64,
        db_token: Option<String>,
        breaker: CircuitBreaker,
        health_tx: mpsc::UnboundedSender<ToolHealthEvent>,
    ) -> Result<Arc<SupervisedIpcRegistry>, ToolHostError> {
        let binary_path =
            Self::find_tool_binary(name).ok_or_else(|| ToolHostError::ExecutionFailed {
                message: format!("Tool binary not found for {name}"),
            })?;

        let socket_path: PathBuf = {
            #[cfg(unix)]
            {
                let p = paths::tool_socket_dir().join(format!("ene-tool-{name}.sock"));
                if p.exists() {
                    let _ = std::fs::remove_file(&p);
                }
                p
            }
            #[cfg(windows)]
            {
                PathBuf::from(format!(r"\\.\pipe\ene-tool-{}", name))
            }
        };

        let db_socket_path: PathBuf = {
            #[cfg(unix)]
            {
                paths::tool_socket_dir().join(format!("ene-db-{name}.sock"))
            }
            #[cfg(windows)]
            {
                PathBuf::from(format!(r"\\.\pipe\ene-db-{}", name))
            }
        };

        let mut tool_sandbox = sandbox.clone();
        tool_sandbox.db_socket = Some(db_socket_path.to_string_lossy().to_string());

        tool_sandbox.db_auth_token = db_token;

        let child = std::process::Command::new(&binary_path)
            .env("ENE_TOOL_SOCKET", &socket_path)
            .spawn()
            .map_err(|e| ToolHostError::ExecutionFailed {
                message: format!("Failed to spawn '{}': {}", binary_path.display(), e),
            })?;

        let process = ToolProcess {
            name: name.to_string(),
            child,
            socket_path: socket_path.clone(),
            binary_path: binary_path.clone(),
            sandbox: tool_sandbox.clone(),
            tool_config: tool_config.clone(),
            restart_count: 0,
        };

        let process = Arc::new(Mutex::new(process));

        let registry = match Self::connect_with_retry(
            &socket_path,
            &tool_sandbox,
            tool_config,
            CONNECT_RETRIES,
            CONNECT_DELAY_MS,
            timeout_ms,
        )
        .await
        {
            Ok(reg) => reg,
            Err(e) => {
                let mut guard = process.lock().await;
                let _ = guard.child.kill();
                let _ = guard.child.wait();
                return Err(ToolHostError::ExecutionFailed {
                    message: format!("Failed to connect to tool '{name}' on startup: {e}"),
                });
            }
        };

        Ok(Arc::new(SupervisedIpcRegistry {
            process,
            registry: std::sync::RwLock::new(Arc::new(registry)),
            timeout_ms,
            breaker: Mutex::new(breaker),
            health_tx: Some(health_tx),
        }))
    }

    fn find_tool_binary(name: &str) -> Option<PathBuf> {
        let exe_suffix = if cfg!(windows) { ".exe" } else { "" };

        let builtin_dir = paths::builtin_tools_dir();
        let user_dir = paths::user_tools_dir();

        let candidates = [
            builtin_dir.join(format!("ene-tool-{name}{exe_suffix}")),
            builtin_dir.join(format!("{name}{exe_suffix}")),
            user_dir.join(format!("ene-tool-{name}{exe_suffix}")),
            user_dir.join(format!("{name}{exe_suffix}")),
        ];

        for candidate in &candidates {
            if candidate.is_file() {
                return Some(candidate.clone());
            }
        }

        None
    }

    /// Attempts to connect to a tool binary with retry logic.
    pub(crate) async fn connect_with_retry(
        socket_path: &Path,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
        max_retries: u32,
        delay_ms: u64,
        timeout_ms: u64,
    ) -> Result<IpcToolRegistry, ToolError> {
        let mut attempts = 0_u32;
        loop {
            match IpcToolRegistry::new(
                socket_path.to_path_buf(),
                sandbox.clone(),
                tool_config.clone(),
                timeout_ms,
            )
            .await
            {
                Ok(registry) => return Ok(registry),
                Err(e) => {
                    attempts = attempts.saturating_add(1);
                    if attempts >= max_retries {
                        return Err(ToolError::ExecutionFailed {
                            message: format!(
                                "Failed to connect to tool at {} after {} attempts: {}",
                                socket_path.display(),
                                attempts,
                                e
                            ),
                        });
                    }
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that the exponential-backoff delay used
    /// between restart attempts grows geometrically up to
    /// the `MAX_DELAY_MS` ceiling, so a flaky tool does not
    /// spin-restart at full CPU.
    #[test]
    fn delay_for_restart_grows_then_caps() {
        assert_eq!(delay_for_restart(0), Duration::from_millis(500));
        assert_eq!(delay_for_restart(1), Duration::from_secs(1));
        assert_eq!(delay_for_restart(2), Duration::from_secs(2));
        assert_eq!(delay_for_restart(3), Duration::from_secs(4));
        assert_eq!(delay_for_restart(4), Duration::from_secs(8));
        // Saturates at MAX_DELAY_MS for very high counts.
        assert_eq!(delay_for_restart(30), Duration::from_millis(MAX_DELAY_MS));
    }
}
