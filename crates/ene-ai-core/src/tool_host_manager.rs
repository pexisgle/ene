use crate::config::AiSettings;
use crate::ipc_client::IpcToolRegistry;
use crate::memory::store::MemoryStore;
use crate::paths;
use crate::tools::definition::ToolRegistry;
use crate::tools::ToolDefinition;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const MAX_RESTARTS: usize = 5;
const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 30_000;
const CONNECT_RETRIES: u32 = 50;
const CONNECT_DELAY_MS: u64 = 50;

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

        eprintln!(
            "[ToolHostManager] Restarting tool '{}' (attempt {}/{})",
            self.name, self.restart_count, MAX_RESTARTS
        );

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

struct ToolEntry {
    process: Arc<Mutex<ToolProcess>>,
    registry: IpcToolRegistry,
}

pub struct ToolHostManager {
    entries: Vec<ToolEntry>,
    extra_registries: Vec<Arc<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
    store: Option<Arc<MemoryStore>>,
}

fn compute_tool_version_hash(tool: &ToolDefinition) -> String {
    use std::hash::{Hash, Hasher};
    let mut state = std::collections::hash_map::DefaultHasher::new();
    tool.name.hash(&mut state);
    tool.description.hash(&mut state);
    tool.keywords.hash(&mut state);
    tool.parameters.to_string().hash(&mut state);
    format!("{:x}", state.finish())
}

#[async_trait::async_trait]
impl ToolRegistry for ToolHostManager {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        for entry in &self.entries {
            tools.extend(entry.registry.list_tools());
        }
        for registry in &self.extra_registries {
            tools.extend(registry.list_tools());
        }
        tools
    }

    fn list_relevant_tools(
        &self,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        let all_tools = self.list_tools();
        let (Some(emb), Some(store)) = (query_embedding, self.store.as_ref()) else {
            return all_tools;
        };

        let search_results = match store.search_tools(emb, limit, 0.0) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("[ToolRAG] Failed to search tools: {}", e);
                return all_tools;
            }
        };

        let all_map: HashMap<String, ToolDefinition> =
            all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();

        search_results
            .into_iter()
            .filter_map(|(name, _similarity)| all_map.get(&name).cloned())
            .collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        match self.tool_index.get(name) {
            Some(&idx) => {
                let num_entries = self.entries.len();
                if idx < num_entries {
                    Self::call_with_supervision(&self.entries[idx], name, arguments).await
                } else {
                    let extra_idx = idx - num_entries;
                    self.extra_registries[extra_idx].call_tool(name, arguments).await
                }
            }
            None => Err(format!("Tool '{}' not found", name)),
        }
    }

    async fn set_session_id(&self, session_id: &str) {
        for entry in &self.entries {
            entry.registry.set_session_id(session_id).await;
        }
        for registry in &self.extra_registries {
            registry.set_session_id(session_id).await;
        }
    }

    async fn ensure_index_built(
        &self,
        embedder: &dyn crate::embedding::EmbeddingProvider,
        store: Option<&MemoryStore>,
    ) -> Result<(), String> {
        let Some(store) = store.or(self.store.as_ref().map(|s| s.as_ref())) else {
            return Ok(());
        };
        let Some(own_store) = &self.store else {
            return Ok(());
        };

        let cached: HashMap<String, (String, Vec<f32>)> = match store.list_tool_embeddings() {
            Ok(entries) => entries
                .into_iter()
                .map(|(name, hash, emb)| (name, (hash, emb)))
                .collect(),
            Err(e) => {
                tracing::warn!("[ToolRAG] Failed to load cached embeddings: {}", e);
                HashMap::new()
            }
        };

        let mut indexed = 0usize;
        let mut reused = 0usize;

        for tool in self.list_tools() {
            let current_hash = compute_tool_version_hash(&tool);

            if let Some((stored_hash, _emb)) = cached.get(&tool.name) {
                if stored_hash == &current_hash {
                    reused += 1;
                    continue;
                }
            }

            let text = tool.embedding_text();
            match embedder.embed(&text).await {
                Ok(embedding) => {
                    if let Err(e) = own_store.upsert_tool_embedding(&tool.name, &current_hash, &embedding) {
                        tracing::warn!("[ToolRAG] Failed to persist embedding for '{}': {}", tool.name, e);
                    }
                    indexed += 1;
                }
                Err(e) => {
                    tracing::warn!("[ToolRAG] Failed to embed tool '{}': {}", tool.name, e);
                }
            }
        }

        tracing::info!(
            "[ToolRAG] Indexed {} tools ({} new, {} reused from DB)",
            indexed + reused,
            indexed,
            reused
        );
        Ok(())
    }
}

impl ToolHostManager {
    pub async fn start(settings: &AiSettings) -> Result<Self, String> {
        let undo_db_path = Some(settings.resolve_undo_db_path().to_string_lossy().to_string());
        let sandbox = settings.sandbox.to_sandbox_config_data(undo_db_path);
        let mut entries = Vec::new();
        let mut tool_index = HashMap::new();

        std::fs::create_dir_all(paths::tool_socket_dir())
            .map_err(|e| format!("Failed to create socket dir: {e}"))?;

        for (name, entry) in &settings.tools.tools {
            if !entry.enable {
                continue;
            }
            let tool_config = match &entry.config {
                serde_json::Value::Object(m) if m.is_empty() => None,
                _ => Some(entry.config.clone()),
            };
            match Self::start_tool(name, &sandbox, tool_config).await {
                Ok(entry) => {
                    let idx = entries.len();
                    for tool in entry.registry.list_tools() {
                        tool_index.entry(tool.name).or_insert(idx);
                    }
                    entries.push(entry);
                }
                Err(e) => {
                    eprintln!("[ToolHostManager] Failed to start tool '{}': {}", name, e);
                }
            }
        }

        Ok(Self {
            entries,
            extra_registries: Vec::new(),
            tool_index,
            store: None,
        })
    }

    pub fn add_registry(&mut self, registry: Arc<dyn ToolRegistry>) {
        let idx = self.entries.len() + self.extra_registries.len();
        for tool in registry.list_tools() {
            self.tool_index.entry(tool.name).or_insert(idx);
        }
        self.extra_registries.push(registry);
    }

    pub fn with_store(&mut self, store: Arc<MemoryStore>) {
        self.store = Some(store);
    }

    pub fn into_registry(self) -> Arc<dyn ToolRegistry> {
        Arc::new(self)
    }

    async fn start_tool(
        name: &str,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    ) -> Result<ToolEntry, String> {
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

        Ok(ToolEntry { process, registry })
    }

    async fn call_with_supervision(
        entry: &ToolEntry,
        name: &str,
        arguments: &str,
    ) -> Result<String, String> {
        let result = entry.registry.call_tool(name, arguments).await;

        if result.is_ok() {
            return result;
        }

        let mut guard = entry.process.lock().await;

        if guard.is_alive() {
            return result;
        }

        eprintln!(
            "[ToolHostManager] Tool '{}' process is dead, attempting restart",
            guard.name
        );

        let delay = guard.delay_for_restart();
        tokio::time::sleep(delay).await;

        guard.restart()?;

        let socket_path = guard.socket_path.clone();
        let sandbox = guard.sandbox.clone();
        let tool_config = guard.tool_config.clone();

        let new_registry = Self::connect_with_retry(
            &socket_path,
            &sandbox,
            tool_config,
            CONNECT_RETRIES,
            CONNECT_DELAY_MS,
        )
        .await?;

        drop(guard);

        new_registry.call_tool(name, arguments).await
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
        socket_path: &PathBuf,
        sandbox: &ene_tool_proto::SandboxConfigData,
        tool_config: Option<serde_json::Value>,
        max_retries: u32,
        delay_ms: u64,
    ) -> Result<IpcToolRegistry, String> {
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
}