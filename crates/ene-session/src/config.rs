use ene_config::schemars;

/// Weights for the composite session-split scoring function.
///
/// The split score is computed as:
/// `time * time_factor + topic * topic_distance + context * context_pressure + turn_count * turn_factor`
///
/// A split is triggered when the total score ≥ `threshold`.
/// All weight fields ideally sum to ≤ 1.0, though this is not enforced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct SplitScoreWeights {
    /// Weight for the time-elapsed factor (0.0–1.0, default 0.35).
    pub time: f32,
    /// Weight for the topic-distance factor — `1 − cosine_similarity` (0.0–1.0, default 0.40).
    pub topic: f32,
    /// Weight for the context-pressure factor — `history_len / max_history` (0.0–1.0, default 0.20).
    pub context: f32,
    /// Weight for the turn-count factor (0.0–1.0, default 0.05).
    pub turn_count: f32,
    /// Score threshold above which a split is triggered (default 0.65).
    pub threshold: f32,
}

impl Default for SplitScoreWeights {
    fn default() -> Self {
        Self {
            time: 0.35,
            topic: 0.40,
            context: 0.20,
            turn_count: 0.05,
            threshold: 0.65,
        }
    }
}

/// Configuration for conversation summarization.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, Default, PartialEq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct SummarizationConfig {
    /// Model used for summarization (uses the chat model if empty).
    pub model: String,
    /// Base URL used for summarization (uses the chat base URL if empty).
    pub base_url: String,
}

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

ene_config::define_config!(
    settings,
    "session",
    /// Configuration for session auto-splitting and summarization behavior.
    pub struct SessionConfig {
        /// Whether to enable automatic session splitting.
        pub auto_split: bool = true,
        /// Time-based split threshold (minutes) — contributes to the time factor.
        /// A full score of 1.0 is reached once this many minutes have elapsed.
        pub timeout_minutes: u64 = 30,
        /// Maximum number of conversation turns to keep in the in-memory history.
        /// Used to compute context pressure. Default is 20 (= 40 messages).
        pub max_history_turns: usize = 20,
        /// Minimum number of turns required before a split may be triggered.
        pub min_turns_before_split: usize = 3,
        /// Maximum number of summaries to inject into the prompt.
        pub recall_limit: usize = 3,
        /// Embedding similarity threshold for `search_summaries`.
        pub similarity_threshold: f32 = 0.5,
        /// Composite split scoring weights.
        pub split_weights: SplitScoreWeights,
        /// Summarization model configuration.
        pub summarization: SummarizationConfig,
    }
);
