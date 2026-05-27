use crate::error::EneCoreError;
use ene_config::EneSettings;
use ene_session::{
    ConversationSession, PendingSplitTask, SplitTaskInput, spawn_split_task,
};
use ene_tool_host::{McpToolRegistry, ToolHostManager, ToolRegistry};
use std::sync::Arc;

/// ene Core runtime state.
///
/// This struct acts as the primary runtime container for an active ene session,
/// including its active user configuration, conversation history, the underlying tool registries,
/// and any pending session splitting tasks.
pub struct EneRuntime {
    /// The active workspace configurations.
    pub settings: EneSettings,
    /// The session holding the conversation history, character details, and memory connection.
    pub session: ConversationSession,
    /// The registry containing active tools (IPC tool host processes, MCP servers, etc.).
    pub registry: Arc<dyn ToolRegistry>,
    /// Any active background task evaluating session boundaries for auto-splitting.
    pub pending_split: Option<PendingSplitTask>,
}

impl EneRuntime {
    /// Initializes a new ene Core runtime with the provided workspace settings.
    ///
    /// This will automatically boot up:
    /// - The requested embedding provider (local Candle or remote OpenAI-compatible API).
    /// - The vector database memory store (if enabled in settings).
    /// - The default character card defined in the settings.
    /// - The tool manager, starting external IPC tool providers and connecting stdio/HTTP MCP servers.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding initialization, database connection, or tool manager start fails.
    pub async fn init(settings: EneSettings) -> Result<Self, EneCoreError> {
        let mut session = ConversationSession::new();

        // 1. Initialize embedding
        let embedder = init_embedding(&settings).map_err(|e| EneCoreError::EmbeddingError(e))?;
        session.memory.embedding_provider = Some(embedder.clone());

        // 2. Initialize memory store if enabled
        let mem_config = settings.get_section::<ene_memory::MemoryConfig>("memory")
            .map_err(|e| EneCoreError::ConfigError(format!("Failed to load memory config: {}", e)))?;

        if mem_config.enabled {
            let store = init_memory_store(&settings, &*embedder).map_err(|e| {
                EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e))
            })?;
            session.memory.memory_store = Some(store);
        }

        // 3. Load default character card
        if let Err(e) = session.load_card(&settings.character) {
            tracing::warn!("Failed to load default character card: {}", e);
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

    /// Evaluates if the current conversation session has reached a semantic boundary.
    /// If so, spawns a background tokio thread to execute an auto session-split,
    /// generating a compiled memory summary and key facts.
    pub fn check_and_perform_split(&mut self, user_input: &str, user_name: &str) {
        let session_config = match self.settings.get_section::<ene_session::SessionConfig>("session") {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load session config for split checking: {}", e);
                return;
            }
        };
        let mem_config = match self.settings.get_section::<ene_memory::MemoryConfig>("memory") {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load memory config for split checking: {}", e);
                return;
            }
        };

        if !mem_config.enabled || !session_config.auto_session_split {
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
                    session_config,
                    summarization_model: self.settings.get_section::<ene_memory::MemoryConfig>("memory").unwrap_or_default().resolve_summarization_model(),
                    summarization_base_url: self.settings.get_section::<ene_memory::MemoryConfig>("memory").unwrap_or_default().resolve_summarization_base_url().unwrap_or_default(),
                    api_key: self.settings.get_section::<crate::config::ProviderSettings>("provider").unwrap_or_default().resolve_api_key(),
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

    /// Computes and caches the embedding vector for a given user query string.
    /// This is typically called prior to storing or searching within the semantic memory store.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if the underlying embedding provider is not initialized
    /// or if the connection/computation fails.
    pub async fn embed_input(&mut self, input: &str) -> Result<Vec<f32>, EneCoreError> {
        let embedder = self
            .session
            .memory
            .embedding_provider
            .clone()
            .ok_or_else(|| {
                EneCoreError::EmbeddingError("No embedding provider initialized".to_string())
            })?;

        let embedding = embedder
            .embed_query(input)
            .await
            .map_err(|e| EneCoreError::EmbeddingError(format!("Failed to embed: {}", e)))?;

        self.session.set_pending_embedding(embedding.clone());
        self.session.set_last_input_embedding(embedding.clone());
        Ok(embedding)
    }
}

/// Builds the active composite tool registry based on workspace config settings.
///
/// This starts up local tool subprocesses managed by `ToolHostManager` and initiates
/// connections to external MCP servers (via stdio transports).
///
/// # Errors
///
/// Returns `EneCoreError` if initialization fails completely.
pub async fn build_tool_registry(
    settings: &EneSettings,
) -> Result<Arc<dyn ToolRegistry>, EneCoreError> {
    let mut manager = match ToolHostManager::start(settings).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("[ToolHostManager] Failed to start tool host, falling back to empty tools: {}", e);
            let mut fallback_settings = settings.clone();
            let fallback_tools = ene_tool_host::ToolSettings {
                tools: std::collections::HashMap::new(),
                ..Default::default()
            };
            let _ = fallback_settings.set_section("tools", &fallback_tools);
            ToolHostManager::start(&fallback_settings)
            .await
            .map_err(|e2| {
                EneCoreError::ConfigError(format!(
                    "Fatal: Failed to start fallback ToolHostManager: {}",
                    e2
                ))
            })?
        }
    };

    let mcp_servers = settings.get_section::<Vec<ene_tool_host::McpServerConfig>>("mcp_servers").unwrap_or_default();
    if !mcp_servers.is_empty() {
        let mcp = McpToolRegistry::new();
        for server in &mcp_servers {
            if !server.enabled {
                continue;
            }
            match &server.transport {
                ene_tool_host::McpTransport::Stdio { command, args } => {
                    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    if let Err(err) = mcp.connect_stdio(&server.name, command, &args_ref).await {
                        tracing::warn!(
                            "MCP server '{}' failed to connect: {}",
                            server.name, err
                        );
                    }
                }
                ene_tool_host::McpTransport::Http { url } => {
                    tracing::warn!(
                        "MCP HTTP transport not supported yet for '{}' (URL: {})",
                        server.name, url
                    );
                }
            }
        }
        manager.add_registry(Arc::new(mcp));
    }

    Ok(manager.into_registry())
}

fn init_embedding(settings: &EneSettings) -> Result<Arc<dyn ene_embedding::EmbeddingProvider>, String> {
    let embed_config = settings
        .get_section::<ene_embedding::EmbeddingConfig>("embedding")
        .map_err(|e| format!("Failed to load embedding config: {}", e))?;

    let embed_base_url = embed_config
        .resolve_base_url()
        .map_err(|e| format!("Failed to resolve embedding base URL: {}", e))?;

    let embedder = ene_embedding::create_embedding_provider(
        embed_config.provider_type,
        &embed_config.model,
        &embed_base_url,
        &settings.get_section::<crate::config::ProviderSettings>("provider").unwrap_or_default().resolve_api_key(),
        embed_config.dimensions.unwrap_or(768),
        Some(&embed_config.gguf_quantization),
        ene_config::models_dir(),
    )
    .map_err(|e| format!("Failed to create embedding provider: {}", e))?;

    Ok(Arc::from(embedder))
}

fn init_memory_store(
    settings: &EneSettings,
    embedder: &dyn ene_embedding::EmbeddingProvider,
) -> Result<Arc<ene_memory::MemoryStore>, String> {
    let db_path = settings.get_section::<ene_memory::MemoryConfig>("memory").unwrap_or_default().resolve_memory_db_path();

    if let Some(parent) = db_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create memory DB directory: {}", e))?;
        }
    }

    let dims = embedder.dimensions();

    let store = ene_memory::MemoryStore::open(&db_path, dims)
        .map_err(|e| format!("Failed to open memory store: {}", e))?;

    Ok(Arc::new(store))
}
