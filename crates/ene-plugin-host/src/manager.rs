//! Plugin host manager: process supervision, capability routing, and lifecycle.
//!
//! [`PluginHostManager`] discovers plugin binaries, spawns them as child
//! processes, performs the v3 handshake, and routes advertised capabilities
//! (tools, LLM providers) into the appropriate host registries. It also
//! connects to configured MCP servers and exposes their tools alongside
//! plugin-provided tools.

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

/// A supervised plugin process and its IPC connection.
struct SupervisedPlugin {
    name: String,
    child: std::process::Child,
    socket_path: PathBuf,
    binary_path: PathBuf,
    sandbox: ene_plugin_proto::SandboxConfigData,
    plugin_config: Option<serde_json::Value>,
    restart_count: usize,
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

        let child = std::process::Command::new(&self.binary_path)
            .env("ENE_PLUGIN_SOCKET", &self.socket_path)
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
/// Discovers plugin binaries from [`builtin_plugins_dir`](ene_config::builtin_plugins_dir)
/// and [`user_plugins_dir`](ene_config::user_plugins_dir), spawns each as a
/// child process, performs the v3 handshake, and routes capabilities:
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
    /// Discovers and starts all plugin binaries, performing handshakes and
    /// capability routing. Also connects to configured MCP servers.
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

        let plugin_names = discover_plugins();

        for name in &plugin_names {
            let entry = plugin_config.list.get(name);

            // Skip plugins explicitly disabled in configuration.
            if let Some(entry) = entry
                && !entry.enable
            {
                tracing::info!(
                    component = "PluginHostManager",
                    plugin = %name,
                    "Plugin disabled by configuration; skipping"
                );
                continue;
            }

            let entry_config = entry
                .map(|e| e.config.clone())
                .filter(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()));

            // The expected checksum is a *typed* field on the plugin entry
            // (`PluginEntry::checksum`), not part of the flattened `config`
            // JSON. Pass it explicitly so verification actually runs.
            let expected_checksum = entry.and_then(|e| e.checksum.clone());

            match Self::start_plugin(
                name,
                entry_config,
                expected_checksum,
                db_tokens.get(name).cloned(),
            )
            .await
            {
                Ok((plugin, conn)) => {
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
                            .connect_stdio(&server.name, command, &args_ref)
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
        db_token: Option<String>,
    ) -> Result<
        (
            Arc<Mutex<SupervisedPlugin>>,
            Arc<Mutex<IpcPluginConnection>>,
        ),
        PluginHostError,
    > {
        let binary_path = find_plugin_binary(name).ok_or_else(|| PluginHostError::SpawnFailed {
            name: name.to_string(),
            reason: "binary not found".to_string(),
        })?;

        // Verify binary checksum if configured. When no checksum is
        // configured, log a one-time warning on first launch (the documented
        // trust-on-first-use behavior).
        verify_plugin_checksum(name, &binary_path, expected_checksum.as_deref())?;

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

        let child = std::process::Command::new(&binary_path)
            .env("ENE_PLUGIN_SOCKET", &socket_path)
            .spawn()
            .map_err(|e| PluginHostError::SpawnFailed {
                name: name.to_string(),
                reason: e.to_string(),
            })?;

        let conn =
            IpcPluginConnection::connect(&socket_path, sandbox.clone(), plugin_config.clone())
                .await?;

        let plugin = SupervisedPlugin {
            name: name.to_string(),
            child,
            socket_path: socket_path.clone(),
            binary_path: binary_path.clone(),
            sandbox,
            plugin_config,
            restart_count: 0,
        };

        Ok((Arc::new(Mutex::new(plugin)), Arc::new(Mutex::new(conn))))
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
fn find_plugin_binary(name: &str) -> Option<PathBuf> {
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
            let p = plugin.lock().await;
            if p.restart_count >= MAX_RESTARTS {
                tracing::error!(
                    component = "PluginHostManager",
                    plugin = %name,
                    "Plugin exceeded max restarts; disabled"
                );
                drop(health_tx.send(PluginHealthEvent::Disabled { plugin: name }));
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
                tracing::error!(
                    component = "PluginHostManager",
                    plugin = %name,
                    error = %e,
                    "Failed to restart plugin"
                );
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

/// Verifies that the plugin binary at `path` matches the expected SHA-256
/// checksum.
///
/// When `expected` is `None`, logs a one-time warning that the binary is being
/// launched without integrity verification (trust-on-first-use) and returns
/// `Ok`. When `expected` is `Some`, computes the SHA-256 hash of the binary and
/// returns [`PluginHostError::ChecksumMismatch`] if it doesn't match.
fn verify_plugin_checksum(
    plugin_name: &str,
    path: &PathBuf,
    expected: Option<&str>,
) -> Result<(), PluginHostError> {
    let Some(expected) = expected else {
        tracing::warn!(
            component = "PluginHostManager",
            plugin = %plugin_name,
            "Plugin binary launched without checksum verification (trust-on-first-use); \
             set plugins.list.{plugin_name}.checksum to enable integrity checks"
        );
        return Ok(());
    };

    let contents = std::fs::read(path).map_err(|e| PluginHostError::SpawnFailed {
        name: plugin_name.to_string(),
        reason: format!("cannot read binary for checksum verification: {e}"),
    })?;
    let actual = hex::encode(sha2::Sha256::digest(&contents));
    if actual != expected {
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

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise failure messages"
)]
mod tests {
    use super::*;

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

        // Matching checksum passes.
        assert!(verify_plugin_checksum("fake", &bin_path, Some(&real)).is_ok());

        // Mismatched checksum is rejected with ChecksumMismatch.
        let result = verify_plugin_checksum("fake", &bin_path, Some("deadbeef"));
        assert!(matches!(
            result,
            Err(PluginHostError::ChecksumMismatch { .. })
        ));

        // No checksum configured → trust-on-first-use, passes.
        assert!(verify_plugin_checksum("fake", &bin_path, None).is_ok());

        drop(std::fs::remove_dir_all(&dir));
    }
}
