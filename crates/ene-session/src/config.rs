fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    SessionConfig,
    "summarization",
    /// Configuration for conversation summarization.
    pub struct SummarizationConfig {
        /// Model used for summarization (uses the chat model if empty)
        pub model: String = default_string(),
        /// Base URL used for summarization (uses the chat base URL if empty)
        pub base_url: String = default_string(),
    }
);

ene_config::define_config!(
    settings,
    "session",
    /// Configuration for session auto-splitting and summarization behavior.
    pub struct SessionConfig {
        /// Whether to enable automatic session splitting.
        pub auto_split: bool = true,
        /// Time-based split threshold (minutes) — auto-splits if no activity exceeds this duration.
        pub timeout_minutes: u64 = 30,
        /// Embedding similarity threshold for topic change detection (0.0–1.0).
        /// If similarity with the previous input falls below this value, a topic change is detected.
        pub topic_similarity_threshold: f32 = 0.5,
        /// Minimum number of turns before a split (conversations that are too short are not summarized).
        pub min_turns_before_split: usize = 3,
        /// Maximum number of summaries to inject into the prompt.
        pub recall_limit: usize = 3,
        /// Summarization model configuration.
        pub summarization: SummarizationConfig,
    }
);

impl SummarizationConfig {
    /// Resolves the effective summarisation model, falling back to the chat model.
    #[must_use]
    pub fn resolve_summarization_model(&self, fallback_model: &str) -> String {
        if !self.model.trim().is_empty() {
            return self.model.clone();
        }
        fallback_model.to_string()
    }

    /// Resolves the effective summarisation base URL, falling back to the provider settings.
    pub fn resolve_summarization_base_url(
        &self,
        fallback_url: &str,
    ) -> Result<String, ene_config::ConfigError> {
        if !self.base_url.trim().is_empty() {
            return Ok(self.base_url.clone());
        }
        if !fallback_url.trim().is_empty() {
            return Ok(fallback_url.to_string());
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: String::new(),
        })
    }
}
