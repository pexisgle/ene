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

/// Token budget allocation and context compression settings.
///
/// NOTE: Allocation logic must validate that the sub-budget fields
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
    /// Enable time-based memory decay.
    pub decay_enabled: bool,
    /// Default half-life in days for memory decay.
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

impl Default for CognitionMemoryConfig {
    fn default() -> Self {
        Self {
            write_every_turn: true,
            hybrid_search: true,
            decay_enabled: true,
            default_forgetting_half_life_days: 30.0,
            min_confidence_to_persist: 0.65,
            extraction_timeout_secs: 30,
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
}

impl Default for CharacterMemoryConfig {
    fn default() -> Self {
        Self {
            compile_ccv3_to_semantic_memory: true,
            always_include_identity_kernel: true,
            style_retrieval: true,
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
}
