use crate::error::EneCoreError;
use crate::stream::{EneStreamEvent, run_ene_with_tools};
use ene_config::EneSettings;
use ene_session::{ConversationSession, PendingSplitTask, spawn_split_task};
use ene_tool_host::{CompositeToolRegistry, ToolHostManager, ToolRegistry};
use std::sync::Arc;
use tokio_stream::Stream;

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
    /// Creates a new ene Core runtime with empty defaults.
    ///
    /// Call [`load_settings`](Self::load_settings) or [`apply_settings`](Self::apply_settings)
    /// afterwards to initialize the embedding provider, memory store, character card,
    /// and tool registry.
    pub async fn init() -> Result<Self, EneCoreError> {
        Ok(Self {
            settings: EneSettings::default(),
            session: ConversationSession::new(),
            registry: Arc::new(CompositeToolRegistry::new(vec![])),
            pending_split: None,
        })
    }

    /// Loads the full workspace settings from disk and initializes all subsystems
    /// (embedding provider, memory store, character card, tool registry).
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding initialization, database connection,
    /// or tool manager start fails.
    pub async fn load_settings(&mut self) -> Result<(), EneCoreError> {
        let settings = ene_config::load_full_settings();
        self.apply_settings(settings).await?;
        Ok(())
    }

    /// Applies externally constructed settings and initializes all subsystems
    /// (embedding provider, memory store, character card, tool registry).
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding initialization, database connection,
    /// or tool manager start fails.
    pub async fn apply_settings(&mut self, settings: EneSettings) -> Result<(), EneCoreError> {
        self.settings = settings;

        // 1. Initialize embedding
        let embedder = init_embedding(&self.settings).map_err(|e| EneCoreError::EmbeddingError(e))?;
        self.session.memory.embedding_provider = Some(embedder.clone());

        // 2. Initialize memory store if enabled
        let mem_config = self
            .settings
            .get_section::<ene_memory::MemoryConfig>("memory")
            .map_err(|e| {
                EneCoreError::ConfigError(format!("Failed to load memory config: {}", e))
            })?;

        if mem_config.enabled {
            let store = init_memory_store(&self.settings, &*embedder).map_err(|e| {
                EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e))
            })?;
            self.session.memory.memory_store = Some(store);
        }

        // 3. Load default character card
        if let Err(e) = self.session.load_card(&self.settings.character) {
            tracing::warn!("Failed to load default character card: {}", e);
        }

        // 4. Build tool registry
        self.registry = build_tool_registry(&self.settings).await?;

        Ok(())
    }

    /// Evaluates if the current conversation session has reached a semantic boundary.
    /// If so, spawns a background tokio thread to execute an auto session-split,
    /// generating a compiled memory summary and key facts.
    pub fn check_and_perform_split(&mut self, user_input: &str, user_name: &str) {
        let mem_config = match self
            .settings
            .get_section::<ene_memory::MemoryConfig>("memory")
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load memory config for split checking: {}", e);
                return;
            }
        };
        let session_config = match self
            .settings
            .get_section::<ene_session::SessionConfig>("session")
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load session config for split checking: {}", e);
                return;
            }
        };

        if !mem_config.enabled || !session_config.auto_session_split {
            return;
        }

        if self.pending_split.is_none() {
            let api_key = self
                .settings
                .get_section::<crate::config::ProviderSettings>("provider")
                .unwrap_or_default()
                .resolve_api_key();

            if let Some(input) =
                self.session
                    .prepare_split_input(&self.settings, user_input, user_name, &api_key)
            {
                spawn_split_task(&mut self.pending_split, input);
            }
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

    /// Runs a full AI streaming completion for the given user input.
    ///
    /// This method handles:
    /// - Session boundary detection and auto-splitting.
    /// - Embedding the user input for semantic memory search.
    /// - Recording the user turn and adding the message to history.
    /// - Streaming the AI response with tool calling support.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding, LLM initialization, or stream setup fails.
    pub async fn run(
        &mut self,
        user_input: &str,
    ) -> Result<impl Stream<Item = EneStreamEvent> + 'static, EneCoreError> {
        self.check_and_perform_split(user_input, &self.settings.user_name.clone());
        self.embed_input(user_input).await?;
        self.session.record_user_input();
        self.session.add_user_message(user_input);

        run_ene_with_tools(&self.settings, &self.session, user_input, self.registry.clone()).await
    }

    /// Polls for a completed split result and applies it to the session.
    ///
    /// Delegates to [`ConversationSession::apply_pending_split`].
    pub fn apply_pending_split(
        &mut self,
    ) -> Option<Result<ene_session::SplitResult, ene_session::SessionError>> {
        self.session.apply_pending_split(&mut self.pending_split)
    }
}

/// Builds the active composite tool registry based on workspace config settings.
///
/// Delegates to [`ToolHostManager::start_full`] which handles IPC tool spawning,
/// MCP server connection, and fallback logic automatically.
///
/// # Errors
///
/// Returns `EneCoreError` if initialization fails completely.
pub async fn build_tool_registry(
    settings: &EneSettings,
) -> Result<Arc<dyn ToolRegistry>, EneCoreError> {
    ToolHostManager::start_full(settings).await.map_err(|e| {
        EneCoreError::ConfigError(format!("Fatal: Failed to start ToolHostManager: {}", e))
    })
}

fn init_embedding(
    settings: &EneSettings,
) -> Result<Arc<dyn ene_embedding::EmbeddingProvider>, String> {
    let embed_config = settings
        .get_section::<ene_embedding::EmbeddingConfig>("embedding")
        .map_err(|e| format!("Failed to load embedding config: {}", e))?;

    let api_key = settings
        .get_section::<crate::config::ProviderSettings>("provider")
        .unwrap_or_default()
        .resolve_api_key();

    let embedder = embed_config
        .create_provider(&api_key, ene_config::models_dir())
        .map_err(|e| format!("Failed to create embedding provider: {}", e))?;

    Ok(Arc::from(embedder))
}

fn init_memory_store(
    settings: &EneSettings,
    embedder: &dyn ene_embedding::EmbeddingProvider,
) -> Result<Arc<ene_memory::MemoryStore>, String> {
    let db_path = settings
        .get_section::<ene_memory::MemoryConfig>("memory")
        .unwrap_or_default()
        .resolve_memory_db_path();

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
