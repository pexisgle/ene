use crate::error::ToolHostError;
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
use tokio::sync::Mutex;

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

        // Supervision path: the call failed. Hold the process
        // lock through the alive-check and restart so a second
        // concurrent caller cannot trigger a duplicate restart.
        // Release before `connect_with_retry` (which may sleep
        // for seconds) so other callers are not blocked; then
        // install the new registry and retry on the fresh handle.
        let mut guard = self.process.lock().await;

        if guard.is_alive() {
            // Process is alive; the failure was the tool's
            // own response (e.g. a permission denial).
            // Surface the original error to the caller.
            return first_result;
        }

        if guard.restart_count >= MAX_RESTARTS {
            return Err(ToolHostError::ExecutionFailed {
                message: format!(
                    "Tool '{}' has exceeded max restarts ({}) and is disabled",
                    guard.name, MAX_RESTARTS
                ),
            });
        }

        tracing::warn!(
            "[SupervisedIpcRegistry] Tool '{}' process is dead, attempting restart",
            guard.name
        );

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

        // Retry on the freshly installed registry. The process
        // lock is not held across connect/retry; concurrent
        // callers that arrive while the child is already alive
        // after restart surface their original error without a
        // second restart.
        new_reg_arc.call_tool(name, arguments).await
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

        let mut supervised_registries = Vec::new();

        std::fs::create_dir_all(paths::tool_socket_dir()).map_err(|e| {
            ToolHostError::ExecutionFailed {
                message: format!("Failed to create socket dir: {e}"),
            }
        })?;

        let timeout_ms = tool_config.timeout_ms;
        for (name, entry) in &tool_config.list {
            if !entry.enable {
                continue;
            }
            let tool_config = match &entry.config {
                serde_json::Value::Object(m) if m.is_empty() => None,
                _ => Some(entry.config.clone()),
            };
            let sandbox =
                serde_json::from_value::<ene_tool_proto::SandboxConfigData>(entry.config.clone())
                    .unwrap_or_default();
            let db_token = db_tokens.remove(name);
            match Self::start_tool(name, &sandbox, tool_config, timeout_ms, db_token).await {
                Ok(supervised_entry) => {
                    if let Some(schema) = supervised_entry.config_schema().await {
                        let schema_key = format!("{name}_config");
                        register_runtime_schema(&schema_key, schema);
                    }
                    supervised_registries.push(supervised_entry);
                }
                Err(e) => {
                    tracing::error!(component = "ToolHostManager", tool = %name, error = %e, "Failed to start tool");
                }
            }
        }

        let composite = Arc::new(CompositeToolRegistry::try_new(supervised_registries)?);

        Ok(Self { composite })
    }

    /// Starts all tool binaries and MCP servers and returns the unified registry.
    ///
    /// Combines [`start`](Self::start) (IPC tool spawn), MCP server connection,
    /// and registry aggregation into a single call. Includes automatic fallback
    /// to an empty tool set if the primary startup fails.
    pub async fn start_full(
        config: &EneConfig,
        db_tokens: std::collections::HashMap<String, String>,
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
    ) -> Result<Arc<dyn ToolRegistry>, ToolHostError> {
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
