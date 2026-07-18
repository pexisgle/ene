//! Configuration for the Ene mind runtime.
//!
//! Defines `MindConfig` and its sub-sections for context management,
//! memory, emotion, and character processing.

use ene_config::schemars;

// ────────────────────────────────────────────
// Top-level MindConfig — registered under "mind" key in settings.json
// ────────────────────────────────────────────

ene_config::define_config!(
    settings,
    "mind",
    /// Configuration for the Ene mind runtime.
    ///
    /// Controls context budget, memory extraction/retention, emotion processing,
    /// character compilation, and proactive companion speech.
    pub struct MindConfig {
        /// Context and token budget management.
        #[serde(skip_deserializing, default, skip_serializing)]
        #[schemars(skip)]
        pub context: ContextConfig,

        /// Memory extraction, search, and retention settings.
        #[serde(skip_deserializing, default, skip_serializing)]
        #[schemars(skip)]
        pub memory: MindMemoryConfig,

        /// Emotion and expression processing settings.
        pub emotion: EmotionConfig,

        /// Character card compilation settings.
        #[serde(skip_deserializing, default, skip_serializing)]
        #[schemars(skip)]
        pub character: CharacterMemoryConfig,

        /// Proactive companion speech policy (#103).
        pub proactive: ProactiveConfig,
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
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
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
    /// Token budget for style examples from `CCv3` lorebook.
    pub style_example_budget_tokens: usize,
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
pub struct MindMemoryConfig {
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
    /// Tool-result grounding and guardrail settings (#92).
    pub tool_grounding: ToolGroundingConfig,
    /// Maximum number of typed memories requested by recall planning.
    pub recall_result_limit: usize,
    /// Minimum vector similarity for cognitive recall candidates.
    ///
    /// Distinct from `journal_similarity_threshold` — recall uses a more
    /// lenient similarity gate (default 0.35 vs 0.45 for journal) paired with
    /// a stricter hybrid score floor (default 0.20 vs 0.10 for journal).
    pub recall_similarity_threshold: f32,
    /// Minimum hybrid score required for cognitive recall results.
    ///
    /// Distinct from `journal_min_score` — the recall path uses a stricter
    /// cutoff (default 0.20 vs 0.10 for journal) to ensure high-quality
    /// memories for LLM context injection.
    pub recall_min_score: f32,
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
    /// Hybrid scoring component weights (product defaults live here; store only applies them).
    pub hybrid_weights: ene_store::HybridSearchWeights,
    /// Score boost when a candidate is sourced from an active commitment.
    pub commitment_boost: f32,
    /// Maximum pure-recent fallback candidates gathered during hybrid search.
    pub recent_fallback_limit: usize,
    /// Candidate pool size multiplier base for journal / diagnostics search.
    pub journal_candidate_pool_size: usize,
    /// Minimum vector similarity for journal / diagnostics search (#123).
    ///
    /// Distinct from `recall_similarity_threshold` — journal search is
    /// user-facing and uses a stricter similarity gate (default 0.45 vs 0.35
    /// for recall) while accepting a lower hybrid score floor (default 0.10 vs
    /// 0.20 for recall).
    pub journal_similarity_threshold: f32,
    /// Minimum hybrid score for journal / diagnostics search (#123).
    ///
    /// Distinct from `recall_min_score`. The journal defaults to a lower floor
    /// (0.10 vs 0.20 for recall) so user-facing search returns broader results
    /// while the cognitive recall path applies a stricter quality cutoff.
    pub journal_min_score: f32,
}

/// Tool-result grounding and guardrail settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ToolGroundingConfig {
    /// Maximum characters kept for each tool summary stored in memory.
    pub max_summary_chars: usize,
    /// Minimum confidence for tool-derived candidates.
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub min_confidence: f32,
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

impl Default for MindMemoryConfig {
    fn default() -> Self {
        Self {
            default_forgetting_half_life_days: 30.0,
            min_confidence_to_persist: 0.65,
            extraction_timeout_secs: 30,
            tool_grounding: ToolGroundingConfig::default(),
            recall_result_limit: 8,
            recall_similarity_threshold: 0.35,
            recall_min_score: 0.20,
            mmr_lambda: 0.7,
            mmr_duplicate_cluster_threshold: 0.75,
            mmr_min_slots_semantic: 1,
            mmr_min_slots_episodic: 1,
            mmr_min_slots_user_profile: 1,
            mmr_min_slots_commitment: 1,
            mmr_source_diversity_bonus: 0.05,
            require_migration: false,
            hybrid_weights: ene_store::HybridSearchWeights::default(),
            commitment_boost: 0.25,
            recent_fallback_limit: 5,
            journal_candidate_pool_size: 64,
            journal_similarity_threshold: 0.45,
            journal_min_score: 0.10,
        }
    }
}

impl Default for ToolGroundingConfig {
    fn default() -> Self {
        Self {
            max_summary_chars: 500,
            min_confidence: 0.60,
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
    /// Half-life in minutes for affect decay.
    #[serde(
        skip_deserializing,
        default = "default_decay_half_life_minutes",
        skip_serializing
    )]
    #[schemars(skip)]
    pub decay_half_life_minutes: f64,
    /// Minimum seconds between expression changes (hysteresis).
    #[serde(
        skip_deserializing,
        default = "default_expression_hysteresis_seconds",
        skip_serializing
    )]
    #[schemars(skip)]
    pub expression_hysteresis_seconds: f64,
    /// Allow the LLM to propose expression tokens.
    #[serde(
        skip_deserializing,
        default = "default_llm_can_propose_expression",
        skip_serializing
    )]
    #[schemars(skip)]
    pub llm_can_propose_expression: bool,
    /// Treat LLM expression proposals as advisory only (not commands).
    #[serde(
        skip_deserializing,
        default = "default_llm_expression_is_advisory",
        skip_serializing
    )]
    #[schemars(skip)]
    pub llm_expression_is_advisory: bool,
    /// Timeout in seconds for a single LLM affect-classifier call (#88).
    #[serde(
        skip_deserializing,
        default = "default_classifier_timeout_secs",
        skip_serializing
    )]
    #[schemars(skip)]
    pub classifier_timeout_secs: u64,
    /// Minimum classifier confidence to apply LLM affect deltas (#88).
    #[serde(
        skip_deserializing,
        default = "default_classifier_min_confidence",
        skip_serializing
    )]
    #[schemars(skip)]
    pub classifier_min_confidence: f32,
    /// Prompt library language for affect classifier and cognitive output contract (`en` or `ja`).
    pub classifier_language: String,
}

impl Default for EmotionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            decay_half_life_minutes: 30.0,
            expression_hysteresis_seconds: 4.0,
            llm_can_propose_expression: true,
            llm_expression_is_advisory: true,
            classifier_timeout_secs: crate::emotion::classifier::DEFAULT_CLASSIFIER_TIMEOUT_SECS,
            classifier_min_confidence: 0.5,
            classifier_language: "en".into(),
        }
    }
}

const fn default_decay_half_life_minutes() -> f64 {
    30.0
}

const fn default_expression_hysteresis_seconds() -> f64 {
    4.0
}

const fn default_llm_can_propose_expression() -> bool {
    true
}

const fn default_llm_expression_is_advisory() -> bool {
    true
}

const fn default_classifier_timeout_secs() -> u64 {
    crate::emotion::classifier::DEFAULT_CLASSIFIER_TIMEOUT_SECS
}

const fn default_classifier_min_confidence() -> f32 {
    0.5
}

/// Character card compilation settings.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct CharacterMemoryConfig {
    /// Maximum approximate token budget for the Identity Kernel section.
    pub identity_kernel_max_tokens: usize,
}

impl Default for CharacterMemoryConfig {
    fn default() -> Self {
        Self {
            identity_kernel_max_tokens: 400,
        }
    }
}

/// Input sources for proactive speech decisions (#103).
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveSourcesConfig {
    /// Include recent conversation history in the decision context.
    pub conversation: bool,
    /// Include privacy-safe activity / idle / active-window signals.
    pub activity: bool,
    /// Include a short-lived screen text summary (never raw image bytes).
    pub screen_summary: bool,
}

impl Default for ProactiveSourcesConfig {
    fn default() -> Self {
        Self {
            conversation: true,
            activity: true,
            screen_summary: false,
        }
    }
}

impl ProactiveSourcesConfig {
    /// Returns true when at least one source is enabled.
    #[must_use]
    pub const fn any_enabled(&self) -> bool {
        self.conversation || self.activity || self.screen_summary
    }
}

/// Decision confidence threshold for proactive speech (#103).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveDecisionConfig {
    /// Minimum confidence required before generation starts.
    #[serde(deserialize_with = "deserialize_unit_interval")]
    pub min_confidence: f64,
}

impl Default for ProactiveDecisionConfig {
    fn default() -> Self {
        Self {
            min_confidence: 0.55,
        }
    }
}

/// Proactive companion speech policy (#103).
///
/// Default is disabled so existing chat behaviour is unchanged until the user
/// explicitly opts in.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveConfig {
    /// Master switch for proactive companion speech.
    pub enabled: bool,
    /// Observation / decision tick interval in seconds.
    #[serde(deserialize_with = "deserialize_positive_u64")]
    pub interval_seconds: u64,
    /// Suppress decisions until this many seconds after the last user input.
    #[serde(deserialize_with = "deserialize_non_negative_u64")]
    pub min_idle_seconds: u64,
    /// Suppress further proactive speech after a proactive utterance.
    #[serde(deserialize_with = "deserialize_non_negative_u64")]
    pub cooldown_seconds: u64,
    /// Maximum proactive utterances per conversation session.
    #[serde(
        skip_deserializing,
        default = "default_max_turns_per_session",
        skip_serializing
    )]
    #[schemars(skip)]
    pub max_turns_per_session: usize,
    /// Timeout for the lightweight decision call.
    #[serde(
        skip_deserializing,
        default = "default_decision_timeout_seconds",
        skip_serializing
    )]
    #[schemars(skip)]
    pub decision_timeout_seconds: u64,
    /// Timeout for high-quality proactive generation.
    #[serde(
        skip_deserializing,
        default = "default_generation_timeout_seconds",
        skip_serializing
    )]
    #[schemars(skip)]
    pub generation_timeout_seconds: u64,
    /// Per-source enable flags.
    #[serde(skip_deserializing, default, skip_serializing)]
    #[schemars(skip)]
    pub sources: ProactiveSourcesConfig,
    /// Decision confidence gate.
    #[serde(skip_deserializing, default, skip_serializing)]
    #[schemars(skip)]
    pub decision: ProactiveDecisionConfig,
    /// When true, proactive generation may select tools (default false).
    #[serde(skip_deserializing, default, skip_serializing)]
    #[schemars(skip)]
    pub allow_tools: bool,
    /// Max characters of conversation history included in the decision prompt.
    #[serde(
        skip_deserializing,
        default = "default_max_conversation_chars",
        skip_serializing
    )]
    #[schemars(skip)]
    pub max_conversation_chars: usize,
    /// Max characters of activity snapshot text.
    #[serde(
        skip_deserializing,
        default = "default_max_activity_chars",
        skip_serializing
    )]
    #[schemars(skip)]
    pub max_activity_chars: usize,
    /// Max characters of screen summary text.
    #[serde(
        skip_deserializing,
        default = "default_max_screen_summary_chars",
        skip_serializing
    )]
    #[schemars(skip)]
    pub max_screen_summary_chars: usize,
}

impl Default for ProactiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 60,
            min_idle_seconds: 120,
            cooldown_seconds: 300,
            max_turns_per_session: 6,
            decision_timeout_seconds: 15,
            generation_timeout_seconds: 60,
            sources: ProactiveSourcesConfig::default(),
            decision: ProactiveDecisionConfig::default(),
            allow_tools: false,
            max_conversation_chars: 4_000,
            max_activity_chars: 500,
            max_screen_summary_chars: 800,
        }
    }
}

fn deserialize_positive_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: ::ene_config::serde::Deserializer<'de>,
{
    use ::ene_config::serde::Deserialize;
    let value = u64::deserialize(deserializer)?;
    Ok(value.max(1))
}

fn deserialize_non_negative_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: ::ene_config::serde::Deserializer<'de>,
{
    use ::ene_config::serde::Deserialize;
    u64::deserialize(deserializer)
}

const fn default_max_turns_per_session() -> usize {
    6
}

const fn default_decision_timeout_seconds() -> u64 {
    15
}

const fn default_generation_timeout_seconds() -> u64 {
    60
}

const fn default_max_conversation_chars() -> usize {
    4_000
}

const fn default_max_activity_chars() -> usize {
    500
}

const fn default_max_screen_summary_chars() -> usize {
    800
}

#[expect(dead_code, reason = "retained for future proactive schema fields")]
fn deserialize_positive_usize<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: ::ene_config::serde::Deserializer<'de>,
{
    use ::ene_config::serde::Deserialize;
    let value = usize::deserialize(deserializer)?;
    Ok(value.max(1))
}

#[cfg(test)]
#[cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit/integration tests use unwrap/expect for concise assertions"
    )
)]
mod tests {
    use super::*;

    /// Out-of-range `min_confidence_to_persist` values from a
    /// hand-edited config are clamped into `0.0..=1.0` on load rather
    /// than accepted verbatim (issue #95 confidence range guard).
    #[test]
    fn min_confidence_out_of_range_is_clamped() {
        let high: MindMemoryConfig =
            serde_json::from_str(r#"{"min_confidence_to_persist": 2.5}"#).expect("deserialize");
        assert!(
            (high.min_confidence_to_persist - 1.0).abs() < f64::EPSILON,
            "expected clamp to 1.0, got {}",
            high.min_confidence_to_persist
        );

        let low: MindMemoryConfig =
            serde_json::from_str(r#"{"min_confidence_to_persist": -0.4}"#).expect("deserialize");
        assert!(
            (low.min_confidence_to_persist - 0.0).abs() < f64::EPSILON,
            "expected clamp to 0.0, got {}",
            low.min_confidence_to_persist
        );
    }

    #[test]
    fn mmr_float_fields_out_of_range_are_clamped() {
        let cfg: MindMemoryConfig = serde_json::from_str(
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

    #[test]
    fn tool_grounding_min_confidence_out_of_range_is_clamped() {
        let cfg: MindMemoryConfig = serde_json::from_str(
            r#"{
                "tool_grounding": {
                    "min_confidence": 2.0
                }
            }"#,
        )
        .expect("deserialize");
        assert!((cfg.tool_grounding.min_confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn proactive_defaults_are_disabled() {
        let cfg = MindConfig::default();
        assert!(!cfg.proactive.enabled);
        assert!(!cfg.proactive.allow_tools);
        assert_eq!(cfg.proactive.interval_seconds, 60);
        assert!(cfg.proactive.sources.conversation);
        assert!(!cfg.proactive.sources.screen_summary);
    }

    #[test]
    fn proactive_zero_interval_is_clamped_to_one() {
        let cfg: ProactiveConfig =
            serde_json::from_str(r#"{"interval_seconds": 0}"#).expect("deserialize");
        assert_eq!(cfg.interval_seconds, 1);
    }

    #[test]
    fn proactive_decision_confidence_uses_default() {
        let cfg: ProactiveConfig =
            serde_json::from_str(r#"{"decision":{"min_confidence": 1.5}}"#).expect("deserialize");
        assert!((cfg.decision.min_confidence - 0.55).abs() < f64::EPSILON);
    }

    #[test]
    fn emotion_config_skipped_fields_use_struct_defaults() {
        let cfg: EmotionConfig = serde_json::from_str(r#"{"enabled":false}"#).expect("deserialize");
        assert!(!cfg.enabled);
        assert!((cfg.decay_half_life_minutes - 30.0).abs() < f64::EPSILON);
        assert_eq!(cfg.classifier_language, "en");
    }
}
