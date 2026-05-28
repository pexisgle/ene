use crate::error::EneCoreError;
use crate::stream::{EneStreamEvent, run_ene_with_tools};
use ene_config::EneConfig;
use ene_session::{ConversationSession, PendingSplitTask, spawn_split_task};
use ene_tool_host::{CompositeToolRegistry, ToolHostManager, ToolRegistry};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::Stream;

/// ene Core runtime state.
pub struct EneRuntime {
    /// The active workspace configurations.
    pub config: EneConfig,
    /// The session holding the conversation history, character details, and memory connection.
    pub session: ConversationSession,
    /// The registry containing active tools (IPC tool host processes, MCP servers, etc.).
    pub registry: Arc<dyn ToolRegistry>,
    /// Any active background task evaluating session boundaries for auto-splitting.
    pub pending_split: Option<PendingSplitTask>,

    /// Cached per-character settings sections (from character_settings.json).
    per_character_sections: Option<HashMap<String, serde_json::Value>>,
    /// The file path of the cached character_settings.json.
    per_character_config_path: Option<String>,
}

/// A handle for reading, writing, loading, and persisting configuration.
///
/// Obtain via [`EneRuntime::config`].
pub struct ConfigHandle<'a> {
    runtime: &'a mut EneRuntime,
}

impl ConfigHandle<'_> {
    /// Loads config from the default config path and initializes all subsystems
    /// (embedding, memory, tool registry) except the character card.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding, memory store, or tool manager init fails.
    pub async fn load(&mut self) -> Result<(), EneCoreError> {
        let config = ene_config::load_config();
        self.apply(config).await?;
        Ok(())
    }

    /// Re-reads config from the default config path and re-applies them.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding, memory store, or tool manager init fails.
    pub async fn reload(&mut self) -> Result<(), EneCoreError> {
        self.load().await
    }

    /// Applies externally constructed config and initializes embedding, memory,
    /// and tool registry. Does **not** load a character card.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if embedding, memory store, or tool manager init fails.
    pub async fn apply(&mut self, config: EneConfig) -> Result<(), EneCoreError> {
        self.runtime.config = config;

        let embedder =
            init_embedding(&self.runtime.config).map_err(EneCoreError::EmbeddingError)?;
        self.runtime.session.memory.embedding_provider = Some(embedder.clone());

        let mem_config = self
            .runtime
            .config
            .get_section::<ene_memory::MemoryConfig>("memory")
            .map_err(|e| {
                EneCoreError::ConfigError(format!("Failed to load memory config: {}", e))
            })?;

        if mem_config.enabled {
            let store = init_memory_store(&self.runtime.config, &*embedder).map_err(|e| {
                EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e))
            })?;
            self.runtime.session.memory.memory_store = Some(store);
        }

        self.runtime.registry = build_tool_registry(&self.runtime.config).await?;

        Ok(())
    }

    /// Returns a reference to the current config.
    pub fn get(&self) -> &EneConfig {
        &self.runtime.config
    }

    /// Deserializes a sub-section from the config by key.
    ///
    /// Returns `Ok(T::default())` when the key is absent.
    pub fn get_section<T>(&self, key: &str) -> Result<T, EneCoreError>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        self.runtime
            .config
            .get_section::<T>(key)
            .map_err(EneCoreError::Config)
    }

    /// Serializes and inserts a sub-section into the config by key.
    ///
    /// The change is held in memory. Call [`save`](Self::save) to persist.
    pub fn set_section<T>(&mut self, key: &str, section: &T) -> Result<(), EneCoreError>
    where
        T: serde::Serialize,
    {
        self.runtime
            .config
            .set_section(key, section)
            .map_err(EneCoreError::Config)
    }

    /// Persists the current config to the config file on disk.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if writing the config file fails.
    pub fn save(&self) -> Result<(), EneCoreError> {
        ene_config::save_full_config(&self.runtime.config)
            .map_err(|e| EneCoreError::ConfigError(format!("Failed to save config: {}", e)))
    }

    /// Returns the resolved character card file path from the current config.
    pub fn character_path(&self) -> &str {
        &self.runtime.config.character
    }
}

/// A handle for managing the active character: its card, per-character settings sections,
/// and the character path stored in config.
///
/// Obtain via [`EneRuntime::character`].
pub struct CharacterHandle<'a> {
    runtime: &'a mut EneRuntime,
}

impl CharacterHandle<'_> {
    /// Returns the character card path from the current config (`config.character`).
    pub fn get(&self) -> &str {
        &self.runtime.config.character
    }

    /// Sets the character card path in config (memory only).
    ///
    /// Accepts a bare character name (e.g. `"Alicia"`) or a full path.
    /// Call [`save`](Self::save) to persist and [`load`](Self::load) to
    /// load the card and per-character config.
    pub fn set(&mut self, name: &str) {
        self.runtime.config.character = ene_config::resolve_character_path(name);
    }

    /// Loads the character card and per-character config from the path
    /// stored in config (`config.character`).
    ///
    /// On success, the card is accessible via [`card`](Self::card) and
    /// per-character sections via [`get_section`](Self::get_section).
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if the card file cannot be read or parsed.
    pub fn load(&mut self) -> Result<(), EneCoreError> {
        let path = self.runtime.config.character.clone();
        self.runtime.session.load_card(&path).map_err(EneCoreError::Session)?;

        // Load per-character config (section-based with flat fallback) into cache
        let folder = std::path::Path::new(&path)
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let settings_path = ene_config::character_settings_path(&folder);
        let mut sections = HashMap::new();
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            // Try section-based format: { "character_settings": { ... }, ... }
            if let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content) {
                sections = map;
            } else {
                // Fallback: flat CharacterPerConfig → wrap as "character_settings" section
                if let Ok(per) = serde_json::from_str::<ene_config::CharacterPerConfig>(
                    &content,
                ) {
                    if let Ok(val) = serde_json::to_value(&per) {
                        sections.insert("character_settings".to_string(), val);
                    }
                }
            }
        }

        self.runtime.per_character_config_path =
            Some(settings_path.to_string_lossy().to_string());
        self.runtime.per_character_sections = Some(sections);

        Ok(())
    }

    /// Returns a reference to the loaded character card, if any.
    pub fn card(&self) -> Option<&ene_config::CharacterCardV3> {
        self.runtime.session.character_card.as_ref()
    }

    /// Returns the display name of the loaded character.
    pub fn name(&self) -> &str {
        self.runtime.session.card_name()
    }

    /// Returns the file path of the currently loaded card.
    pub fn current_path(&self) -> &str {
        &self.runtime.session.current_card_path
    }

    /// Deserializes a named section from the loaded per-character config.
    ///
    /// Returns `Ok(T::default())` when the key is absent.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if no per-character config are loaded or
    /// if deserialization fails.
    pub fn get_section<T>(&self, key: &str) -> Result<T, EneCoreError>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        let sections = self.runtime.per_character_sections.as_ref().ok_or_else(|| {
            EneCoreError::ConfigError("No per-character config loaded".to_string())
        })?;
        match sections.get(key) {
            Some(val) => serde_json::from_value(val.clone()).map_err(|e| {
                EneCoreError::ConfigError(format!(
                    "Failed to deserialize character section '{}': {}",
                    key, e
                ))
            }),
            None => Ok(T::default()),
        }
    }

    /// Serializes and inserts a named section into the per-character config
    /// cache.
    ///
    /// The change is held in memory. Call [`save`](Self::save) to persist.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if no per-character config are loaded or
    /// if serialization fails.
    pub fn set_section<T>(&mut self, key: &str, section: &T) -> Result<(), EneCoreError>
    where
        T: serde::Serialize,
    {
        let sections = self.runtime.per_character_sections.as_mut().ok_or_else(|| {
            EneCoreError::ConfigError("No per-character config loaded".to_string())
        })?;
        let val = serde_json::to_value(section).map_err(|e| {
            EneCoreError::ConfigError(format!(
                "Failed to serialize character section '{}': {}",
                key, e
            ))
        })?;
        sections.insert(key.to_string(), val);
        Ok(())
    }

    /// Persists the character path to `settings.json` and all per-character
    /// sections to `character_settings.json`.
    ///
    /// # Errors
    ///
    /// Returns `EneCoreError` if writing to either file fails.
    pub fn save(&self) -> Result<(), EneCoreError> {
        ene_config::save_full_config(&self.runtime.config)
            .map_err(|e| EneCoreError::ConfigError(format!("Failed to save config: {}", e)))?;

        if let (Some(sections), Some(settings_path)) = (
            &self.runtime.per_character_sections,
            &self.runtime.per_character_config_path,
        ) {
            if let Ok(json) = serde_json::to_string_pretty(sections) {
                if let Some(parent) = std::path::Path::new(settings_path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(settings_path, json).map_err(|e| {
                    EneCoreError::ConfigError(format!(
                        "Failed to save character config: {}",
                        e
                    ))
                })?;
            }
        }

        Ok(())
    }
}

impl EneRuntime {
    /// Creates a new ene Core runtime with empty defaults.
    ///
    /// Call [`config`](Self::config)`.load()` then [`character`](Self::character)`.load()`
    /// to initialize.
    pub async fn init() -> Result<Self, EneCoreError> {
        Ok(Self {
            config: EneConfig::default(),
            session: ConversationSession::new(),
            registry: Arc::new(CompositeToolRegistry::new(vec![])),
            pending_split: None,
            per_character_sections: None,
            per_character_config_path: None,
        })
    }

    /// Returns a [`ConfigHandle`] for reading, writing, loading, and persisting
    /// configuration.
    pub fn config(&mut self) -> ConfigHandle<'_> {
        ConfigHandle { runtime: self }
    }

    /// Returns a [`CharacterHandle`] for managing the active character: card,
    /// per-character settings sections, and config path.
    pub fn character(&mut self) -> CharacterHandle<'_> {
        CharacterHandle { runtime: self }
    }

    /// Evaluates if the current conversation session has reached a semantic boundary.
    /// If so, spawns a background tokio thread to execute an auto session-split,
    /// generating a compiled memory summary and key facts.
    pub fn check_and_perform_split(&mut self, user_input: &str, user_name: &str) {
        let mem_config = match self
            .config
            .get_section::<ene_memory::MemoryConfig>("memory")
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load memory config for split checking: {}", e);
                return;
            }
        };
        let session_config = match self
            .config
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
                .config
                .get_section::<crate::config::ProviderConfig>("provider")
                .unwrap_or_default()
                .resolve_api_key();

            if let Some(input) =
                self.session
                    .prepare_split_input(&self.config, user_input, user_name, &api_key)
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
        self.check_and_perform_split(user_input, &self.config.user_name.clone());
        self.embed_input(user_input).await?;
        self.session.record_user_input();
        self.session.add_user_message(user_input);

        run_ene_with_tools(
            &self.config,
            &self.session,
            user_input,
            self.registry.clone(),
        )
        .await
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

/// Builds the active composite tool registry based on workspace config.
///
/// Delegates to [`ToolHostManager::start_full`] which handles IPC tool spawning,
/// MCP server connection, and fallback logic automatically.
///
/// # Errors
///
/// Returns `EneCoreError` if initialization fails completely.
pub async fn build_tool_registry(
    config: &EneConfig,
) -> Result<Arc<dyn ToolRegistry>, EneCoreError> {
    ToolHostManager::start_full(config).await.map_err(|e| {
        EneCoreError::ConfigError(format!("Fatal: Failed to start ToolHostManager: {}", e))
    })
}

fn init_embedding(
    config: &EneConfig,
) -> Result<Arc<dyn ene_embedding::EmbeddingProvider>, String> {
    let embed_config = config
        .get_section::<ene_embedding::EmbeddingConfig>("embedding")
        .map_err(|e| format!("Failed to load embedding config: {}", e))?;

    let api_key = config
        .get_section::<crate::config::ProviderConfig>("provider")
        .unwrap_or_default()
        .resolve_api_key();

    let embedder = embed_config
        .create_provider(&api_key, ene_config::models_dir())
        .map_err(|e| format!("Failed to create embedding provider: {}", e))?;

    Ok(Arc::from(embedder))
}

fn init_memory_store(
    config: &EneConfig,
    embedder: &dyn ene_embedding::EmbeddingProvider,
) -> Result<Arc<ene_memory::MemoryStore>, String> {
    let db_path = config
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
