use crate::config::AiSettings;
use crate::ipc_client::IpcToolRegistry;
use crate::paths;
use crate::tools::definition::ToolRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// 管理下のツールホストプロセス
struct ToolProcess {
    name: String,
    child: std::process::Child,
    socket_path: PathBuf,
    binary_path: PathBuf,
    sandbox: ene_tool_proto::SandboxConfigData,
    restart_count: usize,
}

const MAX_RESTARTS: usize = 5;
const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 30_000;

impl ToolProcess {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn restart(&mut self) -> Result<(), String> {
        self.restart_count += 1;
        if self.restart_count > MAX_RESTARTS {
            return Err(format!(
                "Tool '{}' exceeded max restarts ({})",
                self.name, MAX_RESTARTS
            ));
        }

        let _ = self.child.kill();
        let _ = self.child.wait();

        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        let child = std::process::Command::new(&self.binary_path)
            .env("ENE_TOOL_SOCKET", &self.socket_path)
            .spawn()
            .map_err(|e| format!("Failed to restart '{}': {}", self.binary_path.display(), e))?;

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
        eprintln!("[ToolHostManager] Stopping tool '{}'", self.name);
        let _ = self.child.kill();
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

/// ツールバイナリを管理し、IPC で接続するマネージャー
///
/// `start()` で settings に基づいてツールバイナリを発見・起動し、
/// `into_registry()` で `Arc<dyn ToolRegistry>` に変換する。
/// プロセスクラッシュ時は指数バックオフで自動再起動する。
pub struct ToolHostManager {
    processes: Vec<Arc<tokio::sync::Mutex<ToolProcess>>>,
    registries: Vec<Box<dyn ToolRegistry>>,
}

#[async_trait::async_trait]
impl ToolRegistry for ToolHostManager {
    fn list_tools(&self) -> Vec<crate::tools::ToolDefinition> {
        let mut tools = Vec::new();
        for registry in &self.registries {
            tools.extend(registry.list_tools());
        }
        tools
    }

    fn list_relevant_tools(
        &self,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Vec<crate::tools::ToolDefinition> {
        for registry in &self.registries {
            let tools = registry.list_relevant_tools(query_embedding, limit);
            if !tools.is_empty() {
                return tools;
            }
        }
        self.list_tools()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        for registry in &self.registries {
            let tools = registry.list_tools();
            if tools.iter().any(|t| t.name == name) {
                return registry.call_tool(name, arguments).await;
            }
        }
        Err(format!("Tool '{}' not found", name))
    }

    fn set_session_id(&self, session_id: &str) {
        for registry in &self.registries {
            registry.set_session_id(session_id);
        }
    }

    async fn ensure_index_built(
        &self,
        embedder: &dyn crate::embedding::EmbeddingProvider,
        store: Option<&crate::memory::store::MemoryStore>,
    ) -> Result<(), String> {
        for registry in &self.registries {
            registry.ensure_index_built(embedder, store).await?;
        }
        Ok(())
    }
}

impl ToolHostManager {
    pub async fn start(settings: &AiSettings) -> Result<Self, String> {
        let sandbox = settings.sandbox.to_sandbox_config_data();
        let mut processes = Vec::new();
        let mut registries: Vec<Box<dyn ToolRegistry>> = Vec::new();

        std::fs::create_dir_all(paths::tool_socket_dir())
            .map_err(|e| format!("Failed to create socket dir: {e}"))?;

        for name in &settings.tools.enabled {
            match Self::start_tool(name, &sandbox).await {
                Ok((process, registry)) => {
                    processes.push(process);
                    registries.push(registry);
                }
                Err(e) => {
                    eprintln!("[ToolHostManager] Failed to start tool '{}': {}", name, e);
                }
            }
        }

        Ok(Self {
            processes,
            registries,
        })
    }

    async fn start_tool(
        name: &str,
        sandbox: &ene_tool_proto::SandboxConfigData,
    ) -> Result<(Arc<tokio::sync::Mutex<ToolProcess>>, Box<dyn ToolRegistry>), String> {
        let binary_path = Self::find_tool_binary(name)
            .ok_or_else(|| format!("Tool binary '{}' not found", name))?;

        let socket_path = paths::tool_socket_dir().join(format!("ene-tool-{}.sock", name));

        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let child = std::process::Command::new(&binary_path)
            .env("ENE_TOOL_SOCKET", &socket_path)
            .spawn()
            .map_err(|e| format!("Failed to spawn '{}': {}", binary_path.display(), e))?;

        let process = ToolProcess {
            name: name.to_string(),
            child,
            socket_path: socket_path.clone(),
            binary_path: binary_path.clone(),
            sandbox: sandbox.clone(),
            restart_count: 0,
        };

        let process = Arc::new(tokio::sync::Mutex::new(process));
        let registry = Self::connect_with_retry(&socket_path, sandbox, 50, 50).await?;

        Ok((process, registry))
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

    async fn connect_with_retry(
        socket_path: &Path,
        sandbox: &ene_tool_proto::SandboxConfigData,
        max_retries: u32,
        delay_ms: u64,
    ) -> Result<Box<dyn ToolRegistry>, String> {
        let mut attempts = 0;
        loop {
            match IpcToolRegistry::new(socket_path.to_path_buf(), sandbox.clone()).await {
                Ok(registry) => return Ok(Box::new(registry)),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        return Err(format!(
                            "Failed to connect to tool at {} after {} attempts: {}",
                            socket_path.display(),
                            attempts,
                            e
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }

    pub fn add_registry(&mut self, registry: Box<dyn ToolRegistry>) {
        self.registries.push(registry);
    }

    pub fn into_registry(self) -> Arc<dyn ToolRegistry> {
        Arc::new(self)
    }
}