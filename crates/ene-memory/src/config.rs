#![allow(missing_docs)]

fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    "memory",
    /// Configuration for the memory (SQLite-vec) subsystem.
    pub struct MemoryConfig {
        pub enabled: bool = false,
        pub db_path: String = default_string(),
        pub recall_limit: usize = 5,
        pub similarity_threshold: f32 = 0.5,
        pub time_decay_hours: f64 = 24.0,
        pub similarity_weight: f64 = 0.7,
        pub recency_weight: f64 = 0.3,

        // ── Tool RAG ─────────────────────────────────────────────────────────────
        /// Tool RAG settings — dynamically select only user-input-relevant tools to reduce token consumption
        pub tool_rag_enabled: bool = true,
        pub tool_rag_limit: usize = 6,
        /// Tool names that are always included (kept even after RAG filtering)
        pub tool_rag_always_include: Vec<String> = vec![
            "question".to_string(),
            "todo".to_string(),
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
    /// Resolves the effective database path, defaulting to a file next to the character card.
    pub fn resolve_memory_db_path(&self) -> std::path::PathBuf {
        if !self.db_path.trim().is_empty() {
            return std::path::PathBuf::from(&self.db_path);
        }
        let config = ene_config::get_global_config();
        let card_path = std::path::Path::new(&config.character);
        let dir = card_path.parent().unwrap_or(std::path::Path::new("."));
        dir.join("memory.db")
    }

    /// Resolves the effective summarisation model, falling back to the chat model.
    pub fn resolve_summarization_model(&self) -> String {
        if !self.summarization_model.trim().is_empty() {
            return self.summarization_model.clone();
        }
        ene_config::get_global_config()
            .get_provider_field("model")
            .unwrap_or_else(|| "gpt-4o-mini".to_string())
    }

    /// Resolves the effective summarisation base URL, falling back to the provider settings.
    pub fn resolve_summarization_base_url(&self) -> Result<String, ene_config::ConfigError> {
        if !self.summarization_base_url.trim().is_empty() {
            return Ok(self.summarization_base_url.clone());
        }
        ene_config::get_global_config()
            .get_provider_field("base_url")
            .ok_or(ene_config::ConfigError::MissingBaseUrl {
                env_var: String::new(),
            })
    }
}
