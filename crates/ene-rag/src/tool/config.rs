//! Tool RAG configuration types (#302, moved from `ene-tool-rag`).

use ene_config::{ConfigTarget, HasConfigKey};
use std::collections::HashMap;

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
    /// Number of candidates to retrieve from the vector index (pre-rerank).
    pub top_k: usize,
    /// Final number of tools returned after reranking and filtering.
    pub final_n: usize,
    /// Reserved for LLM `HyDE` expansion.
    ///
    /// Deprecated: no-op; scheduled for removal.
    #[deprecated(note = "LLM HyDE is disabled (no-op); this knob is scheduled for removal")]
    pub use_hyde: bool,
    /// Whether to cosine-rerank candidates (embedding similarity; no LLM).
    pub use_rerank: bool,
    /// Number of candidates to pass to the reranker.
    pub rerank_candidates: usize,
    /// Minimum normalized similarity (`[-1, 1]`) for a tool to be considered (#436).
    pub min_similarity: f32,
    /// Whether to warm the index at startup in a background task.
    pub background_index_on_startup: bool,
    /// Tool names that are always included regardless of relevance.
    pub forced: Vec<String>,
    /// Per-field weighting for the multi-vector similarity computation.
    pub weights: FieldWeightsConfig,
    /// Cap how many tools per category may appear in the result set.
    /// Keys are [`ToolCategory::config_key`](ene_plugin_proto::ToolCategory::config_key)
    /// values (e.g. `"Filesystem"`).
    #[serde(default)]
    pub per_category_limits: HashMap<String, usize>,
}

impl Default for ToolRagConfig {
    #[expect(
        deprecated,
        reason = "initialize deprecated use_hyde until it is removed"
    )]
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: 12,
            final_n: 6,
            use_hyde: false,
            use_rerank: false,
            rerank_candidates: 24,
            min_similarity: 0.25,
            background_index_on_startup: true,
            forced: default_forced(),
            weights: FieldWeightsConfig::default(),
            per_category_limits: HashMap::new(),
        }
    }
}

/// Serializable field weights (#436).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
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
    /// Deprecated soft-penalty weight for the negative embedding.
    ///
    /// Negative examples are now an exclusion gate ([`negative_threshold`](Self::negative_threshold))
    /// rather than a subtracted score; retained for configuration compatibility
    /// and no longer used in scoring.
    pub negative: f32,
    /// Similarity at or above which a tool's negative-example embedding excludes
    /// it from selection (#436). Range `[0, 1]`; `1.0` effectively disables the
    /// gate.
    #[serde(default = "default_negative_threshold")]
    pub negative_threshold: f32,
    /// Weight for the `HyDE` (hypothetical document embedding).
    ///
    /// Deprecated: unused; scheduled for removal.
    #[deprecated(note = "LLM HyDE is disabled; this weight is unused and scheduled for removal")]
    pub hyde: f32,
    /// Weight for the `HyDE` blend factor — the fraction
    /// of the final score contributed by the `HyDE`
    /// similarity, with the remainder from the direct
    /// per-field cosine similarity. Range `[0.0, 1.0]`;
    /// 0.0 disables `HyDE` blending, 1.0 uses only the
    /// `HyDE` similarity.
    ///
    /// Deprecated: unused; scheduled for removal.
    #[deprecated(
        note = "LLM HyDE is disabled; this blend factor is unused and scheduled for removal"
    )]
    #[serde(default = "default_hyde_blend")]
    pub hyde_blend: f32,
}

const fn default_hyde_blend() -> f32 {
    0.6
}

const fn default_negative_threshold() -> f32 {
    0.85
}

impl Default for FieldWeightsConfig {
    #[expect(
        deprecated,
        reason = "initialize deprecated HyDE weight fields until they are removed"
    )]
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            capability: 0.8,
            example: 0.4,
            negative: -0.5,
            negative_threshold: default_negative_threshold(),
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
        ene_config::register_config_schema::<ToolRagConfig>(ConfigTarget::Settings, Some("tools"));
    }
};
