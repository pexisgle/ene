use ene_config::{ConfigTarget, HasConfigKey};

fn default_forced() -> Vec<String> {
    vec![
        "utility.question".to_string(),
        "utility.todo_add".to_string(),
        "utility.get_current_time".to_string(),
    ]
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
/// Tool RAG pipeline configuration.
pub struct ToolRagConfig {
    /// Whether Tool RAG is enabled.
    pub enabled: bool,
    /// Number of candidates to retrieve from the vector index.
    pub top_k: usize,
    /// Final number of tools returned after reranking and filtering.
    pub final_n: usize,
    /// Whether to use Hypothetical Document Embedding to expand the query.
    pub use_hyde: bool,
    /// Whether to use LLM-based reranking on the candidate set.
    pub use_rerank: bool,
    /// Number of candidates to pass to the reranker.
    pub rerank_candidates: usize,
    /// Minimum similarity score for a tool to be considered.
    pub min_similarity: f32,
    /// Whether to warm the index at startup in a background task.
    pub background_index_on_startup: bool,
    /// Tool names that are always included regardless of relevance.
    pub forced: Vec<String>,
    /// Per-field weighting for the multi-vector similarity computation.
    pub weights: FieldWeightsConfig,
}

impl Default for ToolRagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: 12,
            final_n: 6,
            use_hyde: true,
            use_rerank: true,
            rerank_candidates: 24,
            min_similarity: 0.25,
            background_index_on_startup: true,
            forced: default_forced(),
            weights: FieldWeightsConfig::default(),
        }
    }
}

/// Serializable field weights.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct FieldWeightsConfig {
    /// Weight for the tool summary embedding.
    pub summary: f32,
    /// Weight for the tool description embedding.
    pub description: f32,
    /// Weight for the tool example embedding.
    pub example: f32,
    /// Weight for the negative/unwanted embedding (penalizes matches).
    pub negative: f32,
    /// Weight for the `HyDE` (hypothetical document embedding).
    pub hyde: f32,
    /// Weight for the `HyDE` blend factor — the fraction
    /// of the final score contributed by the `HyDE`
    /// similarity, with the remainder from the direct
    /// per-field cosine similarity. Range `[0.0, 1.0]`;
    /// 0.0 disables `HyDE` blending, 1.0 uses only the
    /// `HyDE` similarity.
    #[serde(default = "default_hyde_blend")]
    pub hyde_blend: f32,
}

const fn default_hyde_blend() -> f32 {
    0.6
}

impl Default for FieldWeightsConfig {
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            example: 0.4,
            negative: -0.5,
            hyde: 0.7,
            hyde_blend: default_hyde_blend(),
        }
    }
}

impl HasConfigKey for ToolRagConfig {
    const KEY: &'static str = "rag";
    const TARGET: ConfigTarget = ConfigTarget::Settings;
    fn path() -> &'static [&'static str] {
        &["tools", "rag"]
    }
}

const _: () = {
    #[ctor::ctor(unsafe)]
    fn register() {
        ene_config::__register_schema::<ToolRagConfig>(ConfigTarget::Settings, Some("tools"));
    }
};
