use ene_config::{ConfigTarget, HasConfigKey};
use std::collections::HashMap;

fn default_forced() -> Vec<String> {
    vec![
        "utility.question".to_string(),
        "utility.todo_add".to_string(),
        "utility.get_current_time".to_string(),
    ]
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ToolRagConfig {
    pub enabled: bool,
    /// Number of candidates to retrieve from the vector index (pre-rerank).
    pub top_k: usize,
    /// Final number of tools returned after reranking and filtering.
    pub final_n: usize,
    /// Whether to cosine-rerank candidates (embedding similarity; no LLM).
    ///
    /// Defaults to `false`: the weighted field-similarity score is
    /// already normalized and field-count-independent, so the extra
    /// `rerank_candidates` description re-embeddings per query are not worth
    /// their cost by default. Opt in to evaluate reranking on a workload.
    pub use_rerank: bool,
    pub rerank_candidates: usize,
    /// Minimum normalized similarity (`[-1, 1]`) for a tool to be considered.
    pub min_similarity: f32,
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
    /// Whether to down-weight tools with a recent failure memory.
    ///
    /// When enabled, the pipeline reads recent `tool failure:{tool}` memories
    /// through [`ene_core::ToolFailureSignalPort`] and multiplies the relevance
    /// score of a recently-failed tool by [`failure_penalty`](Self::failure_penalty),
    /// so a tool that keeps failing for this character is less likely to be
    /// re-selected. Requires a failure-signal source to be wired in.
    #[serde(default = "default_use_failure_feedback")]
    pub use_failure_feedback: bool,
    /// Score multiplier applied to a tool with a recent failure.
    ///
    /// Range `[0, 1]`; `1.0` disables the penalty. Only applied when
    /// [`use_failure_feedback`](Self::use_failure_feedback) is enabled and the
    /// penalty keeps the tool at or above `min_similarity`.
    #[serde(default = "default_failure_penalty")]
    pub failure_penalty: f32,
}

impl Default for ToolRagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: 12,
            final_n: 6,
            use_rerank: false,
            rerank_candidates: 24,
            min_similarity: 0.20,
            background_index_on_startup: true,
            forced: default_forced(),
            weights: FieldWeightsConfig::default(),
            per_category_limits: HashMap::new(),
            use_failure_feedback: default_use_failure_feedback(),
            failure_penalty: default_failure_penalty(),
        }
    }
}

const fn default_use_failure_feedback() -> bool {
    true
}

const fn default_failure_penalty() -> f32 {
    0.5
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
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
    /// Negative examples are an exclusion gate ([`negative_threshold`](Self::negative_threshold))
    /// rather than a subtracted score; the field is retained for configuration
    /// compatibility and is not used in scoring.
    pub negative: f32,
    /// Similarity at or above which a tool's negative-example embedding excludes
    /// it from selection. Range `[0, 1]`; `1.0` effectively disables the
    /// gate. Default `0.70` (a value this high only fires on clearly matching
    /// negative examples).
    #[serde(default = "default_negative_threshold")]
    pub negative_threshold: f32,
}

const fn default_negative_threshold() -> f32 {
    0.70
}

impl Default for FieldWeightsConfig {
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            capability: 0.8,
            example: 0.4,
            negative: -0.5,
            negative_threshold: default_negative_threshold(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks in the backward-compatibility contract: settings.json files from
    /// older versions still carry `use_hyde` / `weights.hyde` /
    /// `weights.hyde_blend` keys that no longer exist. Unknown keys must be
    /// ignored and missing fields defaulted (no `deny_unknown_fields`, plus
    /// `#[serde(default)]`), so the legacy payload deserializes cleanly and
    /// the defaults hold.
    #[test]
    fn legacy_hyde_keys_deserialize_cleanly_and_defaults_hold() {
        let payload = serde_json::json!({
            "enabled": true,
            "top_k": 20,
            "use_hyde": true,
            "weights": {
                "hyde": 0.7,
                "hyde_blend": 0.6
            }
        });
        let cfg: ToolRagConfig = serde_json::from_value(payload).unwrap();

        assert!(cfg.enabled);
        assert_eq!(cfg.top_k, 20);
        assert!(!cfg.use_rerank);
        assert_eq!(
            cfg,
            ToolRagConfig {
                enabled: true,
                top_k: 20,
                ..ToolRagConfig::default()
            }
        );
        assert_eq!(cfg.weights, FieldWeightsConfig::default());

        let round_tripped = serde_json::to_value(&cfg).unwrap();
        assert!(round_tripped.get("use_hyde").is_none());
        let weights = round_tripped["weights"].as_object().unwrap();
        assert!(!weights.contains_key("hyde"));
        assert!(!weights.contains_key("hyde_blend"));
    }

    /// Direct check that a legacy `FieldWeightsConfig` payload with only
    /// `HyDE` keys present still yields the field defaults.
    #[test]
    fn legacy_field_weights_payload_defaults() {
        let payload = serde_json::json!({ "hyde": 0.7, "hyde_blend": 0.6 });
        let weights: FieldWeightsConfig = serde_json::from_value(payload).unwrap();
        assert_eq!(weights, FieldWeightsConfig::default());
    }
}
