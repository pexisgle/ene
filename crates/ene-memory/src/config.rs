fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    settings,
    "memory",
    /// Configuration for the memory (SQLite-vec) subsystem.
    pub struct MemoryConfig {
        /// Whether memory is enabled.
        pub enabled: bool = false,
        /// Database path.
        pub db_path: String = default_string(),
        /// Recall limit.
        pub recall_limit: usize = 5,
        /// Similarity threshold.
        pub similarity_threshold: f32 = 0.5,
        /// Time decay hours.
        pub time_decay_hours: f64 = 24.0,
        /// Similarity weight.
        pub similarity_weight: f64 = 0.7,
        /// Recency weight.
        pub recency_weight: f64 = 0.3,

        // ── Tool RAG ─────────────────────────────────────────────────────────────
        /// Tool RAG settings — dynamically select only user-input-relevant tools to reduce token consumption
        pub tool_rag_enabled: bool = true,
        /// Tool RAG limit.
        pub tool_rag_limit: usize = 6,
        /// Tool names that are always included (kept even after RAG filtering)
        pub tool_rag_always_include: Vec<String> = vec![
            "question".to_string(),
            "todo_list".to_string(),
            "todo_add".to_string(),
            "todo_update".to_string(),
            "todo_complete".to_string(),
            "todo_delete".to_string(),
            "get_current_time".to_string(),
        ],

        // ── Summarization Model ──────────────────────────────────────────────────
        /// Model used for summarization (uses the chat model if empty)
        pub summarization_model: String = default_string(),
        /// Base URL used for summarization (uses the chat base URL if empty)
        pub summarization_base_url: String = default_string(),
    }
);

impl MemoryConfig {
    /// Resolves the effective database path, defaulting to a file inside the
    /// character's directory (`assets/characters/{name}/memory.db`).
    #[must_use]
    pub fn resolve_memory_db_path(&self, character_name: &str) -> std::path::PathBuf {
        if !self.db_path.trim().is_empty() {
            return std::path::PathBuf::from(&self.db_path);
        }
        ene_config::paths::character_dir(character_name).join("memory.db")
    }

    /// Resolves the effective summarisation model, falling back to the chat model.
    #[must_use]
    pub fn resolve_summarization_model(&self, fallback_model: &str) -> String {
        if !self.summarization_model.trim().is_empty() {
            return self.summarization_model.clone();
        }
        fallback_model.to_string()
    }

    /// Resolves the effective summarisation base URL, falling back to the provider settings.
    pub fn resolve_summarization_base_url(&self, fallback_url: &str) -> Result<String, ene_config::ConfigError> {
        if !self.summarization_base_url.trim().is_empty() {
            return Ok(self.summarization_base_url.clone());
        }
        if !fallback_url.trim().is_empty() {
            return Ok(fallback_url.to_string());
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: String::new(),
        })
    }
}
