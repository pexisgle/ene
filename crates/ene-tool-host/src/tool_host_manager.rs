use crate::ipc_registry::IpcToolRegistry;
use crate::tools::CompositeToolRegistry;
use crate::tools::ToolDefinition;
use crate::tools::registry::ToolRegistry;
use ene_config as paths;
use ene_config::{EneConfig, register_runtime_schema};
use ene_memory::MemoryStore;
use std::path::PathBuf;
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

    fn restart(&mut self) -> Result<(), crate::error::ToolError> {
        self.restart_count += 1;
        if self.restart_count > MAX_RESTARTS {
            return Err(crate::error::ToolError::ToolExecutionError(format!(
                "Tool '{}' exceeded max restarts ({})",
                self.name, MAX_RESTARTS
            )));
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
            .map_err(|e| {
                crate::error::ToolError::ToolExecutionError(format!(
                    "Failed to restart '{}': {}",
                    self.binary_path.display(),
                    e
                ))
            })?;

        self.child = child;
        Ok(())
    }

    fn delay_for_restart(&self) -> Duration {
        let delay_ms = BASE_DELAY_MS * 2u64.saturating_pow(self.restart_count as u32);
        Duration::from_millis(delay_ms.min(MAX_DELAY_MS))
    }
}

impl Drop for ToolProcess {
    fn drop(&mut self) {
        tracing::info!("[ToolHostManager] Stopping tool '{}'", self.name);
        let _ = self.child.kill();
        ene_tool_proto::transport::cleanup_path(&self.socket_path);
    }
}

struct SupervisedIpcRegistry {
    process: Arc<Mutex<ToolProcess>>,
    registry: std::sync::RwLock<Arc<IpcToolRegistry>>,
}

#[async_trait::async_trait]
impl ToolRegistry for SupervisedIpcRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        reg.list_tools()
    }

    async fn select_tools(
        &self,
        embedder: &dyn ene_provider::EmbeddingProvider,
        query: &str,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        reg.select_tools(embedder, query, limit).await
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, crate::error::ToolError> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let result = reg.call_tool(name, arguments).await;

        if result.is_ok() {
            return result;
        }

        let mut guard = self.process.lock().await;

        if guard.is_alive() {
            return result;
        }

        tracing::warn!(
            "[SupervisedIpcRegistry] Tool '{}' process is dead, attempting restart",
            guard.name
        );

        let delay = guard.delay_for_restart();
        tokio::time::sleep(delay).await;

        guard.restart()?;

        let socket_path = guard.socket_path.clone();
        let sandbox = guard.sandbox.clone();
        let tool_config = guard.tool_config.clone();

        let new_registry = ToolHostManager::connect_with_retry(
            &socket_path,
            &sandbox,
            tool_config,
            CONNECT_RETRIES,
            CONNECT_DELAY_MS,
        )
        .await?;

        let new_reg_arc = Arc::new(new_registry);
        {
            let mut reg_guard = self.registry.write().unwrap_or_else(|e| e.into_inner());
            *reg_guard = Arc::clone(&new_reg_arc);
        }

        drop(guard);

        new_reg_arc.call_tool(name, arguments).await
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        reg.config_schema().await
    }

    async fn set_session_id(&self, session_id: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        reg.set_session_id(session_id).await;
    }

    async fn approve_permission(&self, request_id: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        reg.approve_permission(request_id).await;
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let reg = self
            .registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
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
/// Also supports adding external registries (e.g., [`crate::McpToolRegistry`]) and attaching
/// a [`MemoryStore`] for Tool RAG embeddings.
pub struct ToolHostManager {
    composite: Arc<CompositeToolRegistry>,
}

#[async_trait::async_trait]
impl ToolRegistry for ToolHostManager {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.composite.list_tools()
    }

    async fn select_tools(
        &self,
        embedder: &dyn ene_provider::EmbeddingProvider,
        query: &str,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        self.composite.select_tools(embedder, query, limit).await
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, crate::error::ToolError> {
        self.composite.call_tool(name, arguments).await
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        self.composite.config_schema().await
    }

    async fn set_session_id(&self, session_id: &str) {
        self.composite.set_session_id(session_id).await;
    }

    async fn approve_permission(&self, request_id: &str) {
        self.composite.approve_permission(request_id).await;
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.composite.allow_pattern(action, target_pattern).await;
    }

    async fn ensure_index_built(
        &self,
        embedder: &dyn ene_provider::EmbeddingProvider,
        store: Option<&MemoryStore>,
    ) -> Result<(), crate::error::ToolError> {
        self.composite.ensure_index_built(embedder, store).await
    }
}

fn resolve_undo_db_path(config: &EneConfig) -> std::path::PathBuf {
    let memory_config = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default();
    let memory_path = memory_config.resolve_memory_db_path();
    memory_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("undo.db")
}

impl ToolHostManager {
    /// Starts all enabled tool binaries from the settings configuration.
    ///
    /// Creates the socket directory, iterates over enabled tools in `settings.tools`,
    /// spawns each binary as a child process, and connects to it over IPC.
    /// Also registers each tool's config schema in the global runtime registry
    /// and regenerates `settings.schema.json`.
    pub async fn start(config: &EneConfig) -> Result<Self, crate::error::ToolError> {
        let mut sandbox = config
            .get_section::<ene_tool_proto::SandboxConfigData>()
            .unwrap_or_default();
        sandbox.undo_db_path = Some(resolve_undo_db_path(config).to_string_lossy().to_string());
        let mut supervised_registries = Vec::new();

        std::fs::create_dir_all(paths::tool_socket_dir()).map_err(|e| {
            crate::error::ToolError::ToolExecutionError(format!("Failed to create socket dir: {e}"))
        })?;

        let tool_config = config
            .get_section::<crate::config::ToolConfig>()
            .unwrap_or_default();
        for (name, entry) in &tool_config.tools {
            if !entry.enable {
                continue;
            }
            let tool_config = match &entry.config {
                serde_json::Value::Object(m) if m.is_empty() => None,
                _ => Some(entry.config.clone()),
            };
            match Self::start_tool(name, &sandbox, tool_config).await {
                Ok(supervised_entry) => {
                    // Collect config schema and register it in the runtime registry
                    if let Some(schema) = supervised_entry.config_schema().await {
                        let schema_key = format!("{}_config", name);
                        register_runtime_schema(
                            &schema_key,
                            schema,
                            Some("tool_entry".to_string()),
                        );
                    }
                    supervised_registries.push(supervised_entry);
                }
                Err(e) => {
                    tracing::error!("[ToolHostManager] Failed to start tool '{}': {}", name, e);
                }
            }
        }

        // Regenerate schema file to include runtime tool schemas
        if let Ok(schema_json) = ene_config::generate_schema_json() {
            let schema_path = paths::assets_dir().join("settings.schema.json");
            let _ = std::fs::write(&schema_path, schema_json);
        }

        let composite = Arc::new(CompositeToolRegistry::new(supervised_registries));

        Ok(Self { composite })
    }

    /// Starts all tool binaries and MCP servers and returns the unified registry.
    ///
    /// Combines [`start`](Self::start) (IPC tool spawn), MCP server connection,
    /// and registry aggregation into a single call. Includes automatic fallback
    /// to an empty tool set if the primary startup fails.
    pub async fn start_full(
        config: &EneConfig,
    ) -> Result<Arc<dyn ToolRegistry>, crate::error::ToolError> {
        let mut manager = match Self::start(config).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "[ToolHostManager] Failed to start tool host, falling back to empty tools: {}",
                    e
                );
                let mut fallback_config = config.clone();
                let fallback_tools = crate::config::ToolConfig {
                    tools: std::collections::HashMap::new(),
                    ..Default::default()
                };
                let _ = fallback_config.set_section(&fallback_tools);
                Self::start(&fallback_config).await.map_err(|e2| {
                    crate::error::ToolError::ToolExecutionError(format!(
                        "Fatal: Failed to start fallback ToolHostManager: {}",
                        e2
                    ))
                })?
            }
        };

        let mcp_servers = config
            .get_section_by_key::<Vec<crate::mcp_config::McpServerConfig>>("mcp_servers")
            .unwrap_or_default();
        if !mcp_servers.is_empty() {
            let mcp = crate::McpToolRegistry::new();
            for server in &mcp_servers {
                if !server.enabled {
                    continue;
                }
                match &server.transport {
                    crate::mcp_config::McpTransport::Stdio { command, args } => {
                        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
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
            manager.add_registry(Arc::new(mcp));
        }

        Ok(manager.into_registry())
    }

    /// Adds an external registry (e.g., an [`crate::McpToolRegistry`]) to the composite.
    pub fn add_registry(&mut self, registry: Arc<dyn ToolRegistry>) {
        self.composite.add_registry(registry);
    }

    /// Attaches a memory store for Tool RAG embedding indexing.
    pub fn with_store(&mut self, store: Arc<MemoryStore>) {
        self.composite.set_store(store);
    }

    /// Consumes the manager and returns an `Arc<dyn ToolRegistry>` suitable for use
    /// in the streaming engine.
    pub fn into_registry(self) -> Arc<dyn ToolRegistry> {
        Arc::new(self)
    }

    async fn start_tool(
        name: &str,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    ) -> Result<Arc<dyn ToolRegistry>, crate::error::ToolError> {
        let binary_path = Self::find_tool_binary(name).ok_or_else(|| {
            crate::error::ToolError::ToolExecutionError(format!("Tool binary '{}' not found", name))
        })?;

        let socket_path: PathBuf = {
            #[cfg(unix)]
            {
                let p = paths::tool_socket_dir().join(format!("ene-tool-{}.sock", name));
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

        let child = std::process::Command::new(&binary_path)
            .env("ENE_TOOL_SOCKET", &socket_path)
            .spawn()
            .map_err(|e| {
                crate::error::ToolError::ToolExecutionError(format!(
                    "Failed to spawn '{}': {}",
                    binary_path.display(),
                    e
                ))
            })?;

        let process = ToolProcess {
            name: name.to_string(),
            child,
            socket_path: socket_path.clone(),
            binary_path: binary_path.clone(),
            sandbox: sandbox.clone(),
            tool_config: tool_config.clone(),
            restart_count: 0,
        };

        let process = Arc::new(Mutex::new(process));

        let registry = Self::connect_with_retry(
            &socket_path,
            sandbox,
            tool_config,
            CONNECT_RETRIES,
            CONNECT_DELAY_MS,
        )
        .await?;

        Ok(Arc::new(SupervisedIpcRegistry {
            process,
            registry: std::sync::RwLock::new(Arc::new(registry)),
        }))
    }

    fn find_tool_binary(name: &str) -> Option<PathBuf> {
        let exe_suffix = if cfg!(windows) { ".exe" } else { "" };

        let builtin_dir = paths::builtin_tools_dir();
        let user_dir = paths::user_tools_dir();

        let candidates = [
            builtin_dir.join(format!("ene-tools-{}{}", name, exe_suffix)),
            builtin_dir.join(format!("{}{}", name, exe_suffix)),
            user_dir.join(format!("ene-tools-{}{}", name, exe_suffix)),
            user_dir.join(format!("{}{}", name, exe_suffix)),
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
        socket_path: &PathBuf,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
        max_retries: u32,
        delay_ms: u64,
    ) -> Result<IpcToolRegistry, crate::error::ToolError> {
        let mut attempts = 0;
        loop {
            match IpcToolRegistry::new(
                socket_path.to_path_buf(),
                sandbox.clone(),
                tool_config.clone(),
            )
            .await
            {
                Ok(registry) => return Ok(registry),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        return Err(crate::error::ToolError::ToolExecutionError(format!(
                            "Failed to connect to tool at {} after {} attempts: {}",
                            socket_path.display(),
                            attempts,
                            e
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }
}
