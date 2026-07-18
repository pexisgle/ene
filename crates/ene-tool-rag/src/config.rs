use std::collections::HashMap;

fn default_forced() -> Vec<String> {
    vec![
        "utility.question".to_string(),
        "utility.todo_add".to_string(),
        "utility.get_current_time".to_string(),
    ]
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// Tool RAG pipeline configuration (code defaults only; not in public settings schema).
pub struct ToolRagConfig {
    /// Number of candidates to retrieve from the vector index (pre-rerank).
    pub top_k: usize,
    /// Final number of tools returned after reranking and filtering.
    pub final_n: usize,
    /// Number of candidates to pass to the reranker.
    pub rerank_candidates: usize,
    /// Minimum similarity score for a tool to be considered.
    pub min_similarity: f32,
    /// Tool names that are always included regardless of relevance.
    pub forced: Vec<String>,
    /// Per-field weighting for the multi-vector similarity computation.
    pub weights: FieldWeightsConfig,
    /// Cap how many tools per category may appear in the result set.
    /// Keys are [`ToolCategory::config_key`](ene_tool_proto::ToolCategory::config_key)
    /// values (e.g. `"Filesystem"`).
    #[serde(default)]
    pub per_category_limits: HashMap<String, usize>,
}

impl Default for ToolRagConfig {
    fn default() -> Self {
        Self {
            top_k: 12,
            final_n: 6,
            rerank_candidates: 24,
            min_similarity: 0.25,
            forced: default_forced(),
            weights: FieldWeightsConfig::default(),
            per_category_limits: HashMap::new(),
        }
    }
}

/// Serializable field weights.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FieldWeightsConfig {
    /// Weight for the tool summary embedding.
    pub summary: f32,
    /// Weight for the tool description embedding.
    pub description: f32,
    /// Weight for the capability embedding (category + summary + primary keywords).
    pub capability: f32,
    /// Weight for the tool example embedding.
    pub example: f32,
    /// Weight for the negative/unwanted embedding (penalizes matches).
    pub negative: f32,
}

impl Default for FieldWeightsConfig {
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            capability: 0.8,
            example: 0.4,
            negative: -0.5,
        }
    }
}
