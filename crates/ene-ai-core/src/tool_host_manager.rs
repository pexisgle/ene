use crate::config::AiSettings;
use crate::ipc_client::IpcToolRegistry;
use crate::memory::store::MemoryStore;
use crate::paths;
use crate::tools::definition::ToolRegistry;
use crate::tools::CompositeToolRegistry;
use crate::tools::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const MAX_RESTARTS: usize = 5;
const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 30_000;
pub const CONNECT_RETRIES: u32 = 50;
pub const CONNECT_DELAY_MS: u64 = 50;

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

    fn restart(&mut self) -> Result<(), crate::error::AiCoreError> {
        self.restart_count += 1;
        if self.restart_count > MAX_RESTARTS {
            return Err(crate::error::AiCoreError::ToolExecutionError(format!(
                "Tool '{}' exceeded max restarts ({})",
                self.name, MAX_RESTARTS
            )));
        }

        let _ = self.child.kill();
        let _ = self.child.wait();

        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        tracing::warn!(
            "[ToolHostManager] Restarting tool '{}' (attempt {}/{})",
            self.name, self.restart_count, MAX_RESTARTS
        );

        let child = std::process::Command::new(&self.binary_path)
            .env("ENE_TOOL_SOCKET", &self.socket_path)
            .spawn()
            .map_err(|e| crate::error::AiCoreError::ToolExecutionError(format!("Failed to restart '{}': {}", self.binary_path.display(), e)))?;

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
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

struct SupervisedIpcRegistry {
    process: Arc<Mutex<ToolProcess>>,
    registry: std::sync::RwLock<Arc<IpcToolRegistry>>,
}

#[async_trait::async_trait]
impl ToolRegistry for SupervisedIpcRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner()).clone();
        reg.list_tools()
    }

    fn list_relevant_tools(
        &self,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner()).clone();
        reg.list_relevant_tools(query_embedding, limit)
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, crate::error::AiCoreError> {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner()).clone();
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

    async fn set_session_id(&self, session_id: &str) {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner()).clone();
        reg.set_session_id(session_id).await;
    }
}

pub struct ToolHostManager {
    composite: Arc<CompositeToolRegistry>,
}

#[async_trait::async_trait]
impl ToolRegistry for ToolHostManager {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.composite.list_tools()
    }

    fn list_relevant_tools(
        &self,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        self.composite.list_relevant_tools(query_embedding, limit)
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, crate::error::AiCoreError> {
        self.composite.call_tool(name, arguments).await
    }

    async fn set_session_id(&self, session_id: &str) {
        self.composite.set_session_id(session_id).await;
    }

    async fn ensure_index_built(
        &self,
        embedder: &dyn crate::embedding::EmbeddingProvider,
        store: Option<&MemoryStore>,
    ) -> Result<(), crate::error::AiCoreError> {
        self.composite.ensure_index_built(embedder, store).await
    }
}

impl ToolHostManager {
    pub async fn start(settings: &AiSettings) -> Result<Self, crate::error::AiCoreError> {
        let undo_db_path = Some(settings.resolve_undo_db_path().to_string_lossy().to_string());
        let sandbox = settings.sandbox.to_sandbox_config_data(undo_db_path);
        let mut supervised_registries = Vec::new();

        std::fs::create_dir_all(paths::tool_socket_dir())
            .map_err(|e| crate::error::AiCoreError::ToolExecutionError(format!("Failed to create socket dir: {e}")))?;

        for (name, entry) in &settings.tools.tools {
            if !entry.enable {
                continue;
            }
            let tool_config = match &entry.config {
                serde_json::Value::Object(m) if m.is_empty() => None,
                _ => Some(entry.config.clone()),
            };
            match Self::start_tool(name, &sandbox, tool_config).await {
                Ok(supervised_entry) => {
                    supervised_registries.push(supervised_entry);
                }
                Err(e) => {
                    tracing::error!("[ToolHostManager] Failed to start tool '{}': {}", name, e);
                }
            }
        }

        let composite = Arc::new(CompositeToolRegistry::new(supervised_registries));

        Ok(Self { composite })
    }

    pub fn add_registry(&mut self, registry: Arc<dyn ToolRegistry>) {
        self.composite.add_registry(registry);
    }

    pub fn with_store(&mut self, store: Arc<MemoryStore>) {
        self.composite.set_store(store);
    }

    pub fn into_registry(self) -> Arc<dyn ToolRegistry> {
        Arc::new(self)
    }

    async fn start_tool(
        name: &str,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    ) -> Result<Arc<dyn ToolRegistry>, crate::error::AiCoreError> {
        let binary_path = Self::find_tool_binary(name)
            .ok_or_else(|| crate::error::AiCoreError::ToolExecutionError(format!("Tool binary '{}' not found", name)))?;

        let socket_path = paths::tool_socket_dir().join(format!("ene-tool-{}.sock", name));

        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let child = std::process::Command::new(&binary_path)
            .env("ENE_TOOL_SOCKET", &socket_path)
            .spawn()
            .map_err(|e| crate::error::AiCoreError::ToolExecutionError(format!("Failed to spawn '{}': {}", binary_path.display(), e)))?;

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

    pub async fn connect_with_retry(
        socket_path: &PathBuf,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
        max_retries: u32,
        delay_ms: u64,
    ) -> Result<IpcToolRegistry, crate::error::AiCoreError> {
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
                        return Err(crate::error::AiCoreError::ToolExecutionError(format!(
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