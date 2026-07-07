//! Configuration for the Ene Cognitive Runtime.
//!
//! Defines `CognitionConfig` and its sub-sections for context management,
//! memory, emotion, and character processing.

use ene_config::schemars;

// ────────────────────────────────────────────
// Top-level CognitionConfig — registered under "cognition" key in settings.json
// ────────────────────────────────────────────

ene_config::define_config!(
    settings,
    "cognition",
    /// Configuration for the Ene Cognitive Runtime.
    ///
    /// Controls context budget, memory extraction/retention, emotion processing,
    /// and character compilation. Enabled by default.
    pub struct CognitionConfig {
        /// Enable the cognitive runtime. When disabled, the system falls back
        /// to the legacy streaming pipeline.
        pub enabled: bool = true,

        /// Context and token budget management.
        pub context: ContextConfig,

        /// Memory extraction, search, and retention settings.
        pub memory: CognitionMemoryConfig,

        /// Emotion and expression processing settings.
        pub emotion: EmotionConfig,

        /// Character card compilation settings.
        pub character: CharacterMemoryConfig,
    }
);

// ────────────────────────────────────────────
// Emotion engine mode
// ────────────────────────────────────────────

ene_config::define_label_enum!(
    /// Selects the emotion computation strategy.
    pub enum EngineMode {
        /// Rules-based affect with no LLM participation.
        Deterministic => "Deterministic",
        /// Pure LLM-driven emotion inference.
        Llm => "LLM",
        /// Combine deterministic rules with LLM proposals (default).
        #[default]
        Hybrid => "Hybrid",
    }
);

// ────────────────────────────────────────────
// Sub-sections
// ────────────────────────────────────────────

/// Token budget allocation, compression triggers, and rolling summarization.
///
/// NOTE: Allocation logic validates that the sub-budget fields
/// (`scene_summary_tokens`, `memory_budget_tokens`, `semantic_budget_tokens`,
/// `style_example_budget_tokens`) sum to ≤ `max_prompt_tokens` at startup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ContextConfig {
    /// Maximum total prompt tokens across all sections.
    pub max_prompt_tokens: usize,
    /// Number of recent conversation turns to include in the prompt.
    pub recent_turns: usize,
    /// Token budget for the scene/summary section.
    pub scene_summary_tokens: usize,
    /// Token budget for recalled memories.
    pub memory_budget_tokens: usize,
    /// Token budget for semantic (lorebook) memory.
    pub semantic_budget_tokens: usize,
    /// Token budget for style examples from CCv3 lorebook.
    pub style_example_budget_tokens: usize,
    /// Enable rolling context compression instead of session splits (#79).
    pub compression_enabled: bool,
    /// Turn count threshold before scene-level compression runs.
    pub scene_turn_threshold: usize,
    /// Number of scene spans before chapter rollup.
    pub chapter_span_threshold: usize,
    /// Number of chapter spans before arc rollup.
    pub arc_span_threshold: usize,
    /// Timeout in seconds for a single compression summarization call.
    pub compression_timeout_secs: u64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_prompt_tokens: 12_000,
            recent_turns: 8,
            scene_summary_tokens: 800,
            memory_budget_tokens: 1_800,
            semantic_budget_tokens: 1_200,
            style_example_budget_tokens: 600,
            compression_enabled: true,
            scene_turn_threshold: 12,
            chapter_span_threshold: 5,
            arc_span_threshold: 3,
            compression_timeout_secs: 60,
        }
    }
}

/// Memory extraction, search, and lifecycle settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct CognitionMemoryConfig {
    /// Extract and persist memory on every turn.
    pub write_every_turn: bool,
    /// Use hybrid search (vector + recency + salience + confidence).
    pub hybrid_search: bool,
    /// Enable post-turn natural decay (`Active → Faded → Archived`) via
    /// `ForgettingLifecycle` (#76).
    pub decay_enabled: bool,
    /// Half-life in days for lifecycle decay score and recall recency scoring.
    pub default_forgetting_half_life_days: f64,
    /// Minimum confidence threshold for persisting a memory. This is a
    /// probability, so values outside `0.0..=1.0` are clamped on load
    /// (issue #95 confidence range guard).
    #[serde(deserialize_with = "deserialize_unit_interval")]
    pub min_confidence_to_persist: f64,
    /// Timeout in seconds for a single LLM memory-extraction call. When the
    /// provider does not respond within this budget the extraction fails and
    /// the pipeline falls back to deterministic candidates (issue #66).
    pub extraction_timeout_secs: u64,
    /// Use HyDE query expansion for cognitive memory recall. The planner only
    /// records this hint; downstream recall execution performs the provider call.
    pub use_hyde: bool,
    /// Maximum number of typed memories requested by recall planning.
    pub recall_result_limit: usize,
    /// Minimum vector similarity for vector-sourced recall candidates.
    pub recall_similarity_threshold: f32,
    /// Minimum hybrid score required for recalled memory results.
    pub recall_min_score: f32,
    /// Enable optional LLM reranking of hybrid recall candidates.
    pub rerank_enabled: bool,
    /// Maximum number of top hybrid-search candidates sent to the reranker.
    pub rerank_candidate_limit: usize,
    /// Timeout in seconds for a single LLM memory-rerank call. On timeout or
    /// provider failure the pipeline falls back to hybrid search order (#77).
    pub rerank_timeout_secs: u64,
    /// Enable MMR diversification after hybrid search (#78).
    pub mmr_enabled: bool,
    /// MMR relevance-vs-diversity tradeoff in `[0.0, 1.0]`; higher favors relevance.
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub mmr_lambda: f32,
    /// Lexical similarity threshold for duplicate cluster merging (#78).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub mmr_duplicate_cluster_threshold: f32,
    /// Minimum recalled slots reserved for semantic memories (#78).
    pub mmr_min_slots_semantic: usize,
    /// Minimum recalled slots reserved for episodic memories (#78).
    pub mmr_min_slots_episodic: usize,
    /// Minimum recalled slots reserved for user profile memories (#78).
    pub mmr_min_slots_user_profile: usize,
    /// Minimum recalled slots reserved for commitment memories (#78).
    pub mmr_min_slots_commitment: usize,
    /// Bonus added to MMR score when a candidate introduces a new recall source (#78).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub mmr_source_diversity_bonus: f32,
    /// When true, block recall if legacy rows exist and migration is incomplete (#98).
    pub require_migration: bool,
}

/// Clamp a deserialized confidence into the closed unit interval
/// `0.0..=1.0`. Out-of-range user config values are clamped rather
/// than rejected so a bad hand-edit degrades gracefully instead of
/// failing the boot path.
fn deserialize_unit_interval<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: ::ene_config::serde::Deserializer<'de>,
{
    use ::ene_config::serde::Deserialize;
    Ok(f64::deserialize(deserializer)?.clamp(0.0, 1.0))
}

/// Clamp a deserialized `f32` into the closed unit interval `0.0..=1.0`.
fn deserialize_unit_interval_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: ::ene_config::serde::Deserializer<'de>,
{
    use ::ene_config::serde::Deserialize;
    Ok(f32::deserialize(deserializer)?.clamp(0.0, 1.0))
}

impl Default for CognitionMemoryConfig {
    fn default() -> Self {
        Self {
            write_every_turn: true,
            hybrid_search: true,
            decay_enabled: true,
            default_forgetting_half_life_days: 30.0,
            min_confidence_to_persist: 0.65,
            extraction_timeout_secs: 30,
            use_hyde: false,
            recall_result_limit: 8,
            recall_similarity_threshold: 0.35,
            recall_min_score: 0.20,
            rerank_enabled: false,
            rerank_candidate_limit: 16,
            rerank_timeout_secs: 10,
            mmr_enabled: true,
            mmr_lambda: 0.7,
            mmr_duplicate_cluster_threshold: 0.75,
            mmr_min_slots_semantic: 1,
            mmr_min_slots_episodic: 1,
            mmr_min_slots_user_profile: 1,
            mmr_min_slots_commitment: 1,
            mmr_source_diversity_bonus: 0.05,
            require_migration: false,
        }
    }
}

/// Emotion engine and expression arbitration settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct EmotionConfig {
    /// Enable emotion processing.
    pub enabled: bool,
    /// Engine mode.
    pub engine: EngineMode,
    /// Half-life in minutes for affect decay.
    pub decay_half_life_minutes: f64,
    /// Minimum seconds between expression changes (hysteresis).
    pub expression_hysteresis_seconds: f64,
    /// Allow the LLM to propose expression tokens.
    pub llm_can_propose_expression: bool,
    /// Treat LLM expression proposals as advisory only (not commands).
    pub llm_expression_is_advisory: bool,
    /// Timeout in seconds for a single LLM affect-classifier call (#88).
    pub classifier_timeout_secs: u64,
    /// Minimum classifier confidence to apply LLM affect deltas (#88).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub classifier_min_confidence: f32,
    /// Prompt library language for affect classifier and cognitive output contract (`en` or `ja`).
    pub classifier_language: String,
}

impl Default for EmotionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: EngineMode::default(),
            decay_half_life_minutes: 30.0,
            expression_hysteresis_seconds: 4.0,
            llm_can_propose_expression: true,
            llm_expression_is_advisory: true,
            classifier_timeout_secs: 15,
            classifier_min_confidence: 0.5,
            classifier_language: "en".into(),
        }
    }
}

/// Character card compilation settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct CharacterMemoryConfig {
    /// Compile CCv3 lorebook entries into the semantic memory index.
    pub compile_ccv3_to_semantic_memory: bool,
    /// Always include the Identity Kernel at the top of every prompt.
    pub always_include_identity_kernel: bool,
    /// Enable retrieval of character style examples from lorebook.
    pub style_retrieval: bool,
    /// Maximum approximate token budget for the Identity Kernel section.
    pub identity_kernel_max_tokens: usize,
}

impl Default for CharacterMemoryConfig {
    fn default() -> Self {
        Self {
            compile_ccv3_to_semantic_memory: true,
            always_include_identity_kernel: true,
            style_retrieval: true,
            identity_kernel_max_tokens: 400,
        }
    }
}

#[cfg(test)]
#[cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
mod tests {
    use super::*;

    /// Out-of-range `min_confidence_to_persist` values from a
    /// hand-edited config are clamped into `0.0..=1.0` on load rather
    /// than accepted verbatim (issue #95 confidence range guard).
    #[test]
    fn min_confidence_out_of_range_is_clamped() {
        let high: CognitionMemoryConfig =
            serde_json::from_str(r#"{"min_confidence_to_persist": 2.5}"#).expect("deserialize");
        assert!(
            (high.min_confidence_to_persist - 1.0).abs() < f64::EPSILON,
            "expected clamp to 1.0, got {}",
            high.min_confidence_to_persist
        );

        let low: CognitionMemoryConfig =
            serde_json::from_str(r#"{"min_confidence_to_persist": -0.4}"#).expect("deserialize");
        assert!(
            (low.min_confidence_to_persist - 0.0).abs() < f64::EPSILON,
            "expected clamp to 0.0, got {}",
            low.min_confidence_to_persist
        );
    }

    #[test]
    fn mmr_float_fields_out_of_range_are_clamped() {
        let cfg: CognitionMemoryConfig = serde_json::from_str(
            r#"{
                "mmr_lambda": 1.5,
                "mmr_duplicate_cluster_threshold": -0.2,
                "mmr_source_diversity_bonus": 2.0
            }"#,
        )
        .expect("deserialize");
        assert!((cfg.mmr_lambda - 1.0).abs() < f32::EPSILON);
        assert!(cfg.mmr_duplicate_cluster_threshold < f32::EPSILON);
        assert!((cfg.mmr_source_diversity_bonus - 1.0).abs() < f32::EPSILON);
    }
}
