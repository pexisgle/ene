use ene_config::AiSettings;
use ene_session::{ConversationSession, PendingSplitTask, SplitTaskInput, spawn_split_task, init_embedding, init_memory_store};
use ene_tool_host::{ToolRegistry, ToolHostManager, McpToolRegistry};
use crate::error::AiCoreError;
use std::sync::Arc;

pub struct AiRuntime {
    pub settings: AiSettings,
    pub session: ConversationSession,
    pub registry: Arc<dyn ToolRegistry>,
    pub pending_split: Option<PendingSplitTask>,
}

impl AiRuntime {
    pub async fn init(settings: AiSettings) -> Result<Self, AiCoreError> {
        let mut session = ConversationSession::new();

        // 1. Initialize embedding
        let embedder = init_embedding(&settings)
            .map_err(|e| AiCoreError::EmbeddingError(e))?;
        session.memory.embedding_provider = Some(embedder.clone());

        // 2. Initialize memory store if enabled
        if settings.memory.enabled {
            let store = init_memory_store(&settings, &*embedder)
                .map_err(|e| AiCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e)))?;
            session.memory.memory_store = Some(store);
        }

        // 3. Load default character card
        if let Err(e) = session.load_card(&settings.character_card_path) {
            eprintln!("Warning: Failed to load default card: {}", e);
        }

        // 4. Build tool registry
        let registry = build_tool_registry(&settings).await?;

        Ok(Self {
            settings,
            session,
            registry,
            pending_split: None,
        })
    }

    pub fn check_and_perform_split(&mut self, user_input: &str, user_name: &str) {
        if !self.settings.memory.enabled || !self.settings.memory.auto_session_split {
            return;
        }
        let store = match &self.session.memory.memory_store {
            Some(s) => s,
            None => return,
        };
        let embedder = match &self.session.memory.embedding_provider {
            Some(e) => e,
            None => return,
        };
        if self.pending_split.is_none() {
            spawn_split_task(
                &mut self.pending_split,
                SplitTaskInput {
                    last_input_embedding: self.session.state.last_input_embedding.clone(),
                    last_message_time: self.session.state.last_message_time,
                    current_turn_count: self.session.state.current_turn_count,
                    user_input: user_input.to_string(),
                    settings: self.settings.clone(),
                    history: self.session.history.conversation_history.clone(),
                    session_id: self.session.memory.session_id.clone(),
                    card_name: self.session.card_name().to_string(),
                    user_name: user_name.to_string(),
                    store: store.clone(),
                    embedder: embedder.clone(),
                },
            );
        }
    }

    pub async fn embed_input(&mut self, input: &str) -> Result<Vec<f32>, AiCoreError> {
        let embedder = self
            .session
            .memory
            .embedding_provider
            .clone()
            .ok_or_else(|| AiCoreError::EmbeddingError("No embedding provider initialized".to_string()))?;

        let embedding = embedder
            .embed_query(input)
            .await
            .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to embed: {}", e)))?;

        self.session.set_pending_embedding(embedding.clone());
        self.session.set_last_input_embedding(embedding.clone());
        Ok(embedding)
    }
}

pub async fn build_tool_registry(settings: &AiSettings) -> Result<Arc<dyn ToolRegistry>, AiCoreError> {
    let mut manager = match ToolHostManager::start(settings).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[ToolHostManager] Warning: {}", e);
            ToolHostManager::start(&AiSettings {
                tools: ene_config::AiToolSettings {
                    tools: std::collections::HashMap::new(),
                },
                ..settings.clone()
            })
            .await
            .map_err(|e2| AiCoreError::ConfigError(format!("Fatal: Failed to start fallback ToolHostManager: {}", e2)))?
        }
    };

    if !settings.mcp_servers.is_empty() {
        let mcp = McpToolRegistry::new();
        for server in &settings.mcp_servers {
            if !server.enabled {
                continue;
            }
            match &server.transport {
                ene_config::McpTransport::Stdio { command, args } => {
                    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    if let Err(err) = mcp.connect_stdio(&server.name, command, &args_ref).await {
                        eprintln!(
                            "Warning: MCP server '{}' failed to connect: {}",
                            server.name, err
                        );
                    }
                }
                ene_config::McpTransport::Http { url } => {
                    eprintln!(
                        "Warning: MCP HTTP transport not supported yet for '{}': {}",
                        server.name, url
                    );
                }
            }
        }
        manager.add_registry(Arc::new(mcp));
    }

    Ok(manager.into_registry())
}
