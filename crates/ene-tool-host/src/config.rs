use serde::de::DeserializeOwned;
use std::collections::HashMap;

fn default_tools() -> HashMap<String, ToolEntry> {
    ["fs", "web", "browser", "utility", "app"]
        .into_iter()
        .map(|name| (name.to_string(), ToolEntry::default()))
        .collect()
}

ene_config::define_config!(
    "tools",
    /// Global tool subsystem config.
    pub struct ToolConfig {
        /// Whether tool calling is enabled globally.
        pub tool_calling_enabled: bool = true,
        /// Maximum number of sequential tool calls per turn.
        pub max_tool_call_rounds: usize = 10,
        /// Tool call execution timeout in milliseconds.
        pub tool_call_timeout_ms: u64 = 60_000,
        /// Per-tool enable/disable map with optional extra config.
        pub tools: HashMap<String, ToolEntry> = default_tools(),
    }
);

ene_config::define_config!(
    "tool_entry",
    /// A single tool entry in the `tools` map.
    pub struct ToolEntry {
        /// Whether this tool is enabled.
        pub enable: bool = true,
        /// Tool-specific configuration (flattened into the parent).
        #[serde(flatten)]
        pub config: serde_json::Value = serde_json::Value::Object(Default::default()),
    }
);

fn default_forced_tools() -> Vec<String> {
    vec![
        "utility.question".to_string(),
        "utility.todo_add".to_string(),
        "utility.get_current_time".to_string(),
    ]
}

ene_config::define_config!(
    "tool_rag",
    /// Tool RAG pipeline configuration.
    pub struct ToolRagConfig {
        /// Whether Tool RAG is enabled.
        pub enabled: bool = true,
        /// Number of candidates to retrieve from the vector index.
        pub top_k: usize = 12,
        /// Final number of tools returned after reranking and filtering.
        pub final_n: usize = 6,
        /// Whether to use Hypothetical Document Embedding to expand the query.
        pub use_hyde: bool = true,
        /// Whether to use LLM-based reranking on the candidate set.
        pub use_rerank: bool = true,
        /// Number of candidates to pass to the reranker.
        pub rerank_candidates: usize = 24,
        /// Minimum similarity score for a tool to be considered.
        pub min_similarity: f32 = 0.25,
        /// Whether to warm the index at startup in a background task.
        pub background_index_on_startup: bool = false,
        /// Tool names that are always included regardless of relevance.
        pub forced_tools: Vec<String> = default_forced_tools(),
        /// Per-field weighting for the multi-vector similarity computation.
        pub weights: FieldWeightsConfig = FieldWeightsConfig::default(),
    }
);

/// Serializable field weights, config-level counterpart of
/// [`crate::rag::FieldWeights`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct FieldWeightsConfig {
    /// Weight for the tool summary embedding.
    pub summary: f32,
    /// Weight for the tool description embedding.
    pub description: f32,
    /// Weight for the tool capability embedding.
    pub capability: f32,
    /// Weight for the tool example embedding.
    pub example: f32,
    /// Weight for the negative/unwanted embedding (penalizes matches).
    pub negative: f32,
    /// Weight for the HyDE (hypothetical document embedding).
    pub hyde: f32,
}

impl Default for FieldWeightsConfig {
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            capability: 0.8,
            example: 0.4,
            negative: -0.5,
            hyde: 0.7,
        }
    }
}

impl ToolEntry {
    /// Deserializes tool-specific settings in a type-safe manner and supports type completion
    pub fn deserialize_config<T>(&self) -> Result<T, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(self.config.clone())
    }
}
