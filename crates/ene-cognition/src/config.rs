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
// Sub-sections
// ────────────────────────────────────────────

/// Token budget allocation and context compression settings.
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
    /// Minimum confidence threshold for persisting a memory.
    pub min_confidence_to_persist: f64,
}

impl Default for CognitionMemoryConfig {
    fn default() -> Self {
        Self {
            write_every_turn: true,
            hybrid_search: true,
            decay_enabled: true,
            default_forgetting_half_life_days: 30.0,
            min_confidence_to_persist: 0.65,
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
    /// Engine mode: "deterministic", "llm", or "hybrid".
    pub engine: String,
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
            engine: "hybrid".to_string(),
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
