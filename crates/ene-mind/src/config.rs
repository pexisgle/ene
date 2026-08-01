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

        /// Topic-boundary detection policy (#367).
        pub topic_boundary: TopicBoundaryConfig,

        /// Session lifecycle settings (#369).
        pub session: SessionConfig,
    }
);

// ────────────────────────────────────────────
// Sub-sections
// ────────────────────────────────────────────

/// Context window, compression triggers, and rolling summarization.
///
/// Per-section token budgets were removed in #370: prompt packing now fills
/// the model's effective context window (#364) in priority order rather than
/// allocating fixed sub-budgets. The only window-related knob here is
/// `max_prompt_tokens`, an optional operator cap on the prompt window.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ContextConfig {
    /// Optional operator cap on the prompt window, in tokens.
    ///
    /// When set, combined with the model's advertised window as
    /// `min(advertised, max_prompt_tokens)` when computing the effective
    /// context window (#364, #370), so it can only ever *shrink* a model's
    /// stated limit, never exceed it. When `None` (the default) the prompt
    /// auto-follows the model's advertised window. Packing then fills that
    /// window in priority order rather than against per-section sub-budgets.
    pub max_prompt_tokens: Option<usize>,
    /// Number of recent conversation turns to include in the prompt.
    pub recent_turns: usize,
    /// Turn count threshold before scene-level compression runs.
    pub scene_turn_threshold: usize,
    /// History token estimate at or above which window-pressure compression
    /// runs (#368). This is the secondary, token-based safety net that
    /// complements the primary topic-boundary trigger: when a single topic
    /// runs long without a detected boundary, the oldest span is compressed
    /// once the retained history reaches this many estimated tokens. Replaces
    /// the former message-count ratio heuristic.
    pub context_pressure_tokens: usize,
    /// Number of scene spans before chapter rollup.
    pub chapter_span_threshold: usize,
    /// Number of chapter spans before arc rollup.
    pub arc_span_threshold: usize,
    /// Timeout in seconds for a single compression summarization call.
    pub compression_timeout_secs: u64,
    /// Prompt library language for compression summarizer (`en` or `ja`).
    pub compression_language: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_prompt_tokens: None,
            recent_turns: 8,
            scene_turn_threshold: 12,
            context_pressure_tokens: 2_000,
            chapter_span_threshold: 5,
            arc_span_threshold: 3,
            compression_timeout_secs: 60,
            compression_language: "en".into(),
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
    /// Half-life in days for the access-boost recency decay used in hybrid
    /// recall scoring.
    ///
    /// Access history fades on this timescale so old accesses stop boosting a
    /// memory's recall score. Defaults to [`ene_rag::ACCESS_BOOST_HALF_LIFE_DAYS`]
    /// (14.0), shorter than the typical content-forgetting half-life.
    pub access_boost_half_life_days: f64,
    /// Score below which an active memory transitions to faded (#350).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub fade_threshold: f32,
    /// Score below which a faded memory transitions to archived (#350).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub archive_threshold: f32,
    /// Minimum confidence threshold for persisting a memory. This is a
    /// probability, so values outside `0.0..=1.0` are clamped on load
    /// (issue #95 confidence range guard).
    #[serde(deserialize_with = "deserialize_unit_interval")]
    pub min_confidence_to_persist: f64,
    /// Minimum confidence margin by which an incoming candidate must exceed an
    /// existing contradictory memory to *supersede* (replace) it (#352).
    ///
    /// One of the four arbiter thresholds (with [`Self::min_confidence_to_persist`],
    /// [`Self::semantic_similarity_threshold`], and [`Self::dispute_confidence_gap`]).
    /// When the candidate's confidence exceeds the existing memory's by at least
    /// this delta the arbiter replaces the old row; a smaller margin falls
    /// through to dispute / user-confirmation handling.
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub supersede_confidence_delta: f32,
    /// Cosine similarity at or above which two memories are treated as semantic
    /// duplicates during deduplication (#352).
    ///
    /// This value is strongly dependent on the embedding model's similarity
    /// distribution, so it should be re-tuned when the embedding provider is
    /// swapped (the provider layer is designed for exactly that, #318).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub semantic_similarity_threshold: f32,
    /// Confidence gap below which a contradictory candidate marks the existing
    /// memory as *disputed* rather than superseding it or escalating to user
    /// confirmation (#352).
    ///
    /// When the candidate's confidence is within this gap of the existing
    /// memory's (i.e. `candidate − existing > -dispute_confidence_gap`) the
    /// existing memory is flagged disputed; a larger shortfall defers the
    /// decision to user confirmation.
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub dispute_confidence_gap: f32,
    /// Character bigram Jaccard threshold for a relaxed `source_quote` match (#353).
    ///
    /// When normalized substring matching fails, the arbiter compares character
    /// bigrams between the quote and turn text. A score at or above this value
    /// accepts the quote but applies [`Self::source_quote_ngram_confidence_penalty`].
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub source_quote_ngram_threshold: f32,
    /// Confidence subtracted when a `source_quote` passes only via the n-gram
    /// fallback (#353).
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub source_quote_ngram_confidence_penalty: f32,
    /// Timeout in seconds for a single LLM memory-extraction call. When the
    /// provider does not respond within this budget the extraction fails and
    /// the pipeline falls back to deterministic candidates (issue #66).
    pub extraction_timeout_secs: u64,
    /// Minimum title-embedding cosine similarity for the arbiter to treat two
    /// memories as sharing a contradiction key (#351).
    ///
    /// Kind-specific contradiction checks (`Preference`, `UserProfile`,
    /// `Semantic`, `Relationship`, and `Commitment` — see
    /// [`MemoryKind::is_contradiction_kind`](ene_core::MemoryKind::is_contradiction_kind))
    /// decide whether an incoming candidate talks about the *same* subject as
    /// an existing memory by comparing the cosine
    /// similarity of their **title embeddings**. A candidate whose title is at
    /// least this similar to an existing memory of the same kind is checked for
    /// contradiction instead of being persisted as a separate row, so synonymous
    /// titles ("職業" vs "仕事", "住んでいる場所" vs "居住地") collapse into one
    /// subject rather than accumulating contradictory duplicates. When no
    /// embedding provider is available the arbiter falls back to exact
    /// normalized-title equality, so this threshold only applies on the
    /// embedding path.
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub contradiction_title_similarity_threshold: f32,
    /// Tool-result grounding and guardrail settings (#92).
    pub tool_grounding: ToolGroundingConfig,
    /// Maximum number of typed memories requested by recall planning.
    pub recall_result_limit: usize,
    /// Minimum vector similarity for cognitive recall candidates.
    ///
    /// Distinct from `journal_similarity_threshold` — recall uses a more
    /// lenient similarity gate (default 0.35 vs 0.45 for journal) paired with
    /// the same hybrid score floor (default 0.10 for both).
    pub recall_similarity_threshold: f32,
    /// Minimum hybrid score required for cognitive recall results.
    ///
    /// Distinct from `journal_min_score` — the recall path uses a slightly
    /// stricter cutoff (default 0.10 vs 0.05 for journal) to favor
    /// high-quality memories for LLM context injection. The hybrid formula
    /// scales with relevance × quality, so the threshold sits well below the
    /// maximum (a fresh, strongly relevant memory scores ~1.0); the default
    /// keeps recent/lexical-only candidates (`total` ≈ 0.1–0.5) while
    /// excluding unrelated noise.
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
    /// Hybrid scoring component weights (product defaults live here; store only applies them).
    pub hybrid_weights: ene_core::HybridSearchWeights,
    /// Score boost when a candidate is sourced from an active commitment.
    pub commitment_boost: f32,
    /// Minimum title embedding similarity for the commitment ledger to treat two
    /// commitments as the same one (#387).
    ///
    /// The ledger matches incoming commitment candidates against active rows by
    /// comparing the cosine similarity of their **title embeddings**. A candidate
    /// whose title is at least this similar to an existing active commitment
    /// supersedes it instead of being registered as a separate row, so rephrased
    /// promises ("資料をまとめる" vs "資料作成") collapse into one commitment
    /// rather than accumulating contradictory duplicates in the prompt. When no
    /// embedding provider is available the ledger falls back to exact normalized
    /// title equality, so this threshold only applies on the embedding path.
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    pub commitment_title_similarity_threshold: f32,
    /// Maximum pure-recent fallback candidates gathered during hybrid search.
    pub recent_fallback_limit: usize,
    /// Maximum unconfirmed pending candidates that compete in recall.
    ///
    /// Candidates the arbiter deferred with `AskConfirmationLater` sit
    /// in the user-approval queue rather than the typed-memory table, so the
    /// store's gather cannot surface them. Recall loads the live `pending`
    /// queue and lets up to this many candidates compete in the normal hybrid
    /// score competition — topic proximity (lexical overlap with the query)
    /// gates which of them actually surface, so the character only learns of
    /// unconfirmed info when the conversation touches the topic. `0` disables
    /// pending-candidate recall entirely (the settings-screen review list is
    /// unaffected).
    pub recall_pending_candidate_limit: usize,
    /// Candidate pool size multiplier base for journal / diagnostics search.
    pub journal_candidate_pool_size: usize,
    /// Minimum vector similarity for journal / diagnostics search (#123).
    ///
    /// Distinct from `recall_similarity_threshold` — journal search is
    /// user-facing and uses a stricter similarity gate (default 0.45 vs 0.35
    /// for recall) while accepting the same hybrid score floor (default 0.10
    /// for both).
    pub journal_similarity_threshold: f32,
    /// Minimum hybrid score for journal / diagnostics search (#123).
    ///
    /// Distinct from `recall_min_score`. The journal defaults to the same
    /// floor (0.10) so user-facing search returns broad results; the cognitive
    /// recall path can raise it per-character if needed.
    pub journal_min_score: f32,
    /// Self-reflection pipeline configuration (#210).
    #[serde(default)]
    pub reflection: ReflectionConfig,
    /// Pending memory-candidate retention policy (#420).
    #[serde(default)]
    pub pending_candidate_retention: PendingCandidateRetentionConfig,
}

/// Tool-result grounding and guardrail settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ToolGroundingConfig {
    /// Enable grounding tool call results into cognitive memory (#92).
    pub enabled: bool,
    /// Maximum characters kept for each tool summary stored in memory.
    pub max_summary_chars: usize,
    /// Persist successful tool calls as procedure memories.
    ///
    /// Default `false`: lasting value is decided by the post-turn LLM
    /// extractor. Enable only as a deterministic fallback when LLM extraction
    /// is disabled or returns nothing.
    pub persist_success_procedure: bool,
    /// Persist failed tool calls as reflection memories.
    ///
    /// Used as a fallback when LLM extraction does not own the turn; the LLM
    /// path may still choose to persist important failures itself.
    pub persist_failure_reflection: bool,
    /// Persist concise user-visible tool outcomes as episodic memories.
    ///
    /// Default `false` for the same reason as [`Self::persist_success_procedure`].
    pub persist_user_visible_episodic: bool,
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

/// Clamp a value into the closed unit interval `0.0..=1.0`.
/// Non-finite values (`NaN` / ±∞) become `0.0` so a bad hand-edit cannot
/// silently disable comparisons that treat `NaN >= x` as false.
fn clamp_unit_interval_f32(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Clamp a deserialized `f32` into the closed unit interval `0.0..=1.0`.
fn deserialize_unit_interval_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: ::ene_config::serde::Deserializer<'de>,
{
    use ::ene_config::serde::Deserialize;
    Ok(clamp_unit_interval_f32(f32::deserialize(deserializer)?))
}

impl Default for MindMemoryConfig {
    fn default() -> Self {
        Self {
            default_forgetting_half_life_days: 30.0,
            access_boost_half_life_days: ene_rag::ACCESS_BOOST_HALF_LIFE_DAYS,
            fade_threshold: ene_rag::FADE_THRESHOLD,
            archive_threshold: ene_rag::ARCHIVE_THRESHOLD,
            min_confidence_to_persist: 0.65,
            supersede_confidence_delta: 0.05,
            semantic_similarity_threshold: 0.85,
            dispute_confidence_gap: 0.15,
            source_quote_ngram_threshold: 0.85,
            source_quote_ngram_confidence_penalty: 0.15,
            extraction_timeout_secs: 30,
            contradiction_title_similarity_threshold: 0.82,
            tool_grounding: ToolGroundingConfig::default(),
            recall_result_limit: 8,
            recall_similarity_threshold: 0.35,
            recall_min_score: 0.10,
            mmr_lambda: 0.7,
            mmr_duplicate_cluster_threshold: 0.75,
            mmr_min_slots_semantic: 1,
            mmr_min_slots_episodic: 1,
            mmr_min_slots_user_profile: 1,
            mmr_min_slots_commitment: 1,
            mmr_source_diversity_bonus: 0.05,
            hybrid_weights: ene_core::HybridSearchWeights::default(),
            commitment_boost: 0.25,
            commitment_title_similarity_threshold: 0.82,
            recent_fallback_limit: 5,
            recall_pending_candidate_limit: 3,
            journal_candidate_pool_size: 64,
            journal_similarity_threshold: 0.45,
            journal_min_score: 0.10,
            reflection: ReflectionConfig::default(),
            pending_candidate_retention: PendingCandidateRetentionConfig::default(),
        }
    }
}

/// Retention policy for the pending memory-candidate approval queue (#420).
///
/// Candidates now persist to the `pending_candidates` table so they survive
/// restarts; without a policy that table would grow without bound. Both limits
/// are enforced together on the post-turn forgetting sweep
/// (`ForgettingLifecycle`), and each can be disabled independently with `0`.
#[derive(
    Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct PendingCandidateRetentionConfig {
    /// Auto-expire candidates older than this many days (`Faded`-equivalent).
    ///
    /// Applies to candidates of **every status, including unanswered
    /// (`pending`) ones**: a candidate left unreviewed past this age is
    /// dropped so the queue cannot grow without bound even when the count cap
    /// is disabled, and a resolved candidate older than this has no further UI
    /// value anyway. `0` disables age expiry.
    pub max_age_days: u32,
    /// Maximum pending (unresolved) candidates kept per character per user.
    ///
    /// When the live queue exceeds this cap the oldest overflow is dropped.
    /// The cap is scoped to the acting user (plus character-shared rows), so
    /// on a multi-user database one user's overflow cannot evict another
    /// user's candidates. `0` disables the count cap.
    pub max_per_character: usize,
}

impl Default for PendingCandidateRetentionConfig {
    fn default() -> Self {
        Self {
            max_age_days: 14,
            max_per_character: 200,
        }
    }
}

impl Default for ToolGroundingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_summary_chars: 500,
            // Successes are judged by the post-turn LLM extractor by default.
            // These flags are fallbacks when LLM extraction is off/empty/failed.
            persist_success_procedure: false,
            persist_failure_reflection: true,
            persist_user_visible_episodic: false,
            min_confidence: 0.60,
        }
    }
}

/// Self-reflection pipeline configuration (#210).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ReflectionConfig {
    /// Enable the self-reflection pipeline (#210).
    pub enabled: bool,
    /// Minimum number of turns between reflection generation passes.
    pub interval_turns: usize,
    /// Minimum number of recorded outcomes required to trigger reflection.
    pub min_outcomes: usize,
    /// Score multiplier applied to memories matching successful strategies.
    pub success_boost: f32,
    /// Score multiplier applied to memories matching strategies to avoid.
    pub failure_penalty: f32,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_turns: 10,
            min_outcomes: 3,
            success_boost: 1.2,
            failure_penalty: 0.8,
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
///
/// The Identity Kernel budget is no longer a fixed setting: it is derived from
/// the model's available context window at compile time (#386), so there are
/// currently no user-facing fields here. The section is retained as the home
/// for future character-compilation settings.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
#[derive(Default)]
pub struct CharacterMemoryConfig {}

/// Session lifecycle policy (#369).
///
/// Automatic splits are limited to inactivity timeouts and manual requests.
/// Topic changes are handled by compression (#368), not session splits.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct SessionConfig {
    /// Minutes of silence after the previous utterance before the next user
    /// message starts a new session. `0` disables automatic timeout splits.
    pub session_timeout_minutes: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            session_timeout_minutes: 120,
        }
    }
}

/// Topic-boundary detection policy (#367).
///
/// Detection maintains a topic **centroid** (an exponentially weighted moving
/// average of the embeddings belonging to the current topic) and scores each
/// completed turn with a composite of the cosine distance from the centroid,
/// the silence since the previous utterance, and the topic's turn count. A
/// boundary is signalled once the composite score reaches
/// [`Self::boundary_threshold`]. The detector only produces a signal/score;
/// acting on it (compression, session split) is a later stage.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct TopicBoundaryConfig {
    /// Master switch for topic-boundary detection.
    pub enabled: bool,
    /// Composite score (`0.0..=1.0`) at or above which a turn is flagged as a
    /// topic boundary. Session splitting is expected to use a higher threshold
    /// than compression.
    pub boundary_threshold: f32,
    /// Weight of the centroid-distance term in the composite score.
    pub weight_centroid: f32,
    /// Weight of the silence term in the composite score.
    pub weight_silence: f32,
    /// Weight of the topic-length (turn-count) term in the composite score.
    ///
    /// Invariant: this must stay **below** `boundary_threshold`. The topic-length
    /// factor saturates at `1.0` once a topic reaches `max_topic_turns`, so a
    /// weight at or above the threshold would fire a boundary on its own —
    /// turning the soft cap into a hard cap that force-splits every coherent
    /// topic at `max_topic_turns` regardless of drift or silence. Violations are
    /// clamped on deserialize with a warning.
    pub weight_topic_length: f32,
    /// Moving-average blend factor for the centroid: the fraction of a new
    /// qualifying embedding folded in each turn. A small value keeps the
    /// centroid slow-moving so it represents the topic as a whole rather than
    /// chasing the latest utterance.
    pub centroid_alpha: f32,
    /// Utterances shorter than this many characters are treated as
    /// backchannels: they neither score a boundary nor update the centroid, so
    /// a "うん" cannot corrupt the topic model.
    pub min_utterance_chars: usize,
    /// Elapsed silence (seconds) that saturates the silence term to `1.0`.
    pub silence_saturation_secs: u32,
    /// Soft cap on turns per topic; the topic-length term reaches `1.0` here.
    /// An over-long topic is likely to contain multiple topics, so this term is
    /// what makes *gradual* drift detectable even when the centroid keeps
    /// tracking it closely.
    pub max_topic_turns: usize,
}

impl Default for TopicBoundaryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            boundary_threshold: 0.30,
            weight_centroid: 0.60,
            weight_silence: 0.15,
            weight_topic_length: 0.25,
            centroid_alpha: 0.15,
            min_utterance_chars: 6,
            silence_saturation_secs: 300,
            max_topic_turns: 12,
        }
    }
}

/// Intermediate deserialize shape for [`TopicBoundaryConfig`].
///
/// Applies unit-interval clamping per field, then
/// [`From<TopicBoundaryConfigDe>`] enforces
/// `weight_topic_length < boundary_threshold`.
#[derive(serde::Deserialize)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
struct TopicBoundaryConfigDe {
    enabled: bool,
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    boundary_threshold: f32,
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    weight_centroid: f32,
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    weight_silence: f32,
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    weight_topic_length: f32,
    #[serde(deserialize_with = "deserialize_unit_interval_f32")]
    centroid_alpha: f32,
    min_utterance_chars: usize,
    silence_saturation_secs: u32,
    max_topic_turns: usize,
}

impl Default for TopicBoundaryConfigDe {
    fn default() -> Self {
        let d = TopicBoundaryConfig::default();
        Self {
            enabled: d.enabled,
            boundary_threshold: d.boundary_threshold,
            weight_centroid: d.weight_centroid,
            weight_silence: d.weight_silence,
            weight_topic_length: d.weight_topic_length,
            centroid_alpha: d.centroid_alpha,
            min_utterance_chars: d.min_utterance_chars,
            silence_saturation_secs: d.silence_saturation_secs,
            max_topic_turns: d.max_topic_turns,
        }
    }
}

impl From<TopicBoundaryConfigDe> for TopicBoundaryConfig {
    fn from(raw: TopicBoundaryConfigDe) -> Self {
        let mut cfg = Self {
            enabled: raw.enabled,
            boundary_threshold: raw.boundary_threshold,
            weight_centroid: raw.weight_centroid,
            weight_silence: raw.weight_silence,
            weight_topic_length: raw.weight_topic_length,
            centroid_alpha: raw.centroid_alpha,
            min_utterance_chars: raw.min_utterance_chars,
            silence_saturation_secs: raw.silence_saturation_secs,
            max_topic_turns: raw.max_topic_turns,
        };
        // Topic-length alone must not be able to trip the boundary: keep the
        // weight strictly below the threshold (next representable below, floored
        // at 0.0 when the threshold itself is 0.0).
        if cfg.weight_topic_length >= cfg.boundary_threshold {
            let clamped = cfg.boundary_threshold.next_down().max(0.0);
            tracing::warn!(
                component = "MindConfig",
                weight_topic_length = cfg.weight_topic_length,
                boundary_threshold = cfg.boundary_threshold,
                clamped,
                "mind.topic_boundary.weight_topic_length must stay below boundary_threshold; clamping"
            );
            cfg.weight_topic_length = clamped;
        }
        cfg
    }
}

impl<'de> ::ene_config::serde::Deserialize<'de> for TopicBoundaryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: ::ene_config::serde::Deserializer<'de>,
    {
        <TopicBoundaryConfigDe as ::ene_config::serde::Deserialize>::deserialize(deserializer)
            .map(Self::from)
    }
}

/// How much of the focused window's title the activity observer captures (#378).
///
/// Window titles routinely contain private data — document and file names
/// (which can embed customer or project names), page URLs, chat contact
/// names, and email subjects. This text is fed to the proactive-speech
/// decision LLM and, when a cloud provider is configured, leaves the local
/// machine. The level is therefore a privacy knob: it defaults to
/// [`WindowTitleLevel::AppOnly`] (the historical app-name-only behaviour) and
/// higher levels are an explicit opt-in.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum WindowTitleLevel {
    /// Capture only the application name (the historical, most-private
    /// behaviour). The window title is never read.
    #[default]
    AppOnly,
    /// Capture the window title with obvious sensitive fragments removed:
    /// filesystem paths, email addresses, URLs, and digit runs that resemble
    /// phone / account numbers.
    RedactedTitle,
    /// Capture the raw window title verbatim. Only choose this with a local
    /// model; with a cloud provider the full title is sent off-machine.
    FullTitle,
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
    /// Window-title capture level for the activity source (#378).
    ///
    /// Only consulted when [`Self::activity`] is enabled. Defaults to
    /// [`WindowTitleLevel::AppOnly`]; see that type for the privacy
    /// implications of raising it.
    pub window_title_level: WindowTitleLevel,
}

impl Default for ProactiveSourcesConfig {
    fn default() -> Self {
        Self {
            conversation: true,
            activity: true,
            screen_summary: false,
            window_title_level: WindowTitleLevel::default(),
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
///
/// **Note:** Several fields use `skip_deserializing` / `skip_serializing` and are
/// invisible in the JSON schema and settings file. These fields always use struct
/// defaults and are part of a staged rollout — they will be exposed in a future
/// release once the proactive system is fully validated. Affected fields include:
/// `max_turns_per_session`, `decision_timeout_seconds`, `generation_timeout_seconds`,
/// `decision`, `allow_tools`, `max_conversation_chars`, `max_activity_chars`,
/// `max_screen_summary_chars`.
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
    pub min_idle_seconds: u64,
    /// Suppress further proactive speech after a proactive utterance.
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
        clippy::indexing_slicing,
        reason = "unit/integration tests use unwrap/expect and fixed indices for concise assertions"
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
    fn commitment_title_similarity_threshold_out_of_range_is_clamped() {
        let high: MindMemoryConfig =
            serde_json::from_str(r#"{"commitment_title_similarity_threshold": 1.7}"#)
                .expect("deserialize");
        assert!((high.commitment_title_similarity_threshold - 1.0).abs() < f32::EPSILON);

        let low: MindMemoryConfig =
            serde_json::from_str(r#"{"commitment_title_similarity_threshold": -0.3}"#)
                .expect("deserialize");
        assert!(low.commitment_title_similarity_threshold < f32::EPSILON);
    }

    /// The three arbiter thresholds added in #352 clamp out-of-range config
    /// values into `0.0..=1.0` on load, matching `min_confidence_to_persist`.
    #[test]
    fn arbiter_thresholds_out_of_range_are_clamped() {
        let high: MindMemoryConfig = serde_json::from_str(
            r#"{
                "supersede_confidence_delta": 1.7,
                "semantic_similarity_threshold": 2.5,
                "dispute_confidence_gap": 3.0
            }"#,
        )
        .expect("deserialize");
        assert!((high.supersede_confidence_delta - 1.0).abs() < f32::EPSILON);
        assert!((high.semantic_similarity_threshold - 1.0).abs() < f32::EPSILON);
        assert!((high.dispute_confidence_gap - 1.0).abs() < f32::EPSILON);

        let low: MindMemoryConfig = serde_json::from_str(
            r#"{
                "supersede_confidence_delta": -0.4,
                "semantic_similarity_threshold": -0.1,
                "dispute_confidence_gap": -0.9
            }"#,
        )
        .expect("deserialize");
        assert!(low.supersede_confidence_delta < f32::EPSILON);
        assert!(low.semantic_similarity_threshold < f32::EPSILON);
        assert!(low.dispute_confidence_gap < f32::EPSILON);
    }

    /// The arbiter threshold defaults match `ArbiterOptions::default()` so
    /// wiring them through config keeps behaviour identical (#352).
    #[test]
    fn arbiter_threshold_defaults_match_arbiter_options() {
        let cfg = MindMemoryConfig::default();
        assert!((cfg.supersede_confidence_delta - 0.05).abs() < f32::EPSILON);
        assert!((cfg.semantic_similarity_threshold - 0.85).abs() < f32::EPSILON);
        assert!((cfg.dispute_confidence_gap - 0.15).abs() < f32::EPSILON);
    }

    #[test]
    fn contradiction_title_similarity_threshold_out_of_range_is_clamped() {
        let high: MindMemoryConfig =
            serde_json::from_str(r#"{"contradiction_title_similarity_threshold": 1.7}"#)
                .expect("deserialize");
        assert!((high.contradiction_title_similarity_threshold - 1.0).abs() < f32::EPSILON);

        let low: MindMemoryConfig =
            serde_json::from_str(r#"{"contradiction_title_similarity_threshold": -0.3}"#)
                .expect("deserialize");
        assert!(low.contradiction_title_similarity_threshold < f32::EPSILON);
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
    fn proactive_sources_round_trip_in_public_schema() {
        let cfg: ProactiveConfig = serde_json::from_str(
            r#"{
                "enabled": true,
                "interval_seconds": 30,
                "sources": {
                    "conversation": false,
                    "activity": true,
                    "screen_summary": true
                }
            }"#,
        )
        .expect("deserialize");
        assert!(cfg.enabled);
        assert_eq!(cfg.interval_seconds, 30);
        assert!(!cfg.sources.conversation);
        assert!(cfg.sources.activity);
        assert!(cfg.sources.screen_summary);

        let json = serde_json::to_value(&cfg).expect("serialize");
        assert_eq!(json["sources"]["conversation"], false);
        assert_eq!(json["sources"]["screen_summary"], true);
    }

    /// The window-title capture level defaults to the most-private
    /// `app_only` when absent, so existing configs keep the historical
    /// app-name-only behaviour (#378).
    #[test]
    fn proactive_window_title_level_defaults_to_app_only() {
        let cfg: ProactiveConfig =
            serde_json::from_str(r#"{"sources": {"activity": true}}"#).expect("deserialize");
        assert_eq!(cfg.sources.window_title_level, WindowTitleLevel::AppOnly);
        assert_eq!(
            MindConfig::default().proactive.sources.window_title_level,
            WindowTitleLevel::AppOnly
        );
    }

    /// All three window-title levels round-trip through the public schema
    /// using their `snake_case` wire names (#378).
    #[test]
    fn proactive_window_title_level_round_trips() {
        for (raw, level) in [
            ("app_only", WindowTitleLevel::AppOnly),
            ("redacted_title", WindowTitleLevel::RedactedTitle),
            ("full_title", WindowTitleLevel::FullTitle),
        ] {
            let json = format!(r#"{{"sources": {{"window_title_level": "{raw}"}}}}"#);
            let cfg: ProactiveConfig = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(cfg.sources.window_title_level, level);
            let out = serde_json::to_value(&cfg).expect("serialize");
            assert_eq!(out["sources"]["window_title_level"], raw);
        }
    }

    #[test]
    fn proactive_zero_interval_is_clamped_to_one() {
        let cfg: ProactiveConfig =
            serde_json::from_str(r#"{"interval_seconds": 0}"#).expect("deserialize");
        assert_eq!(cfg.interval_seconds, 1);
    }

    #[test]
    fn proactive_decision_out_of_range_confidence_is_clamped() {
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

    #[test]
    fn topic_boundary_defaults_are_sane() {
        let cfg = TopicBoundaryConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.boundary_threshold > 0.0 && cfg.boundary_threshold <= 1.0);
        assert!(cfg.centroid_alpha > 0.0 && cfg.centroid_alpha <= 1.0);
        assert!(cfg.min_utterance_chars > 0);
        assert!(cfg.max_topic_turns > 0);
        // The composite weights are a convex combination.
        let total = cfg.weight_centroid + cfg.weight_silence + cfg.weight_topic_length;
        assert!((total - 1.0).abs() < 1e-5);
    }

    #[test]
    fn topic_boundary_out_of_range_fields_are_clamped() {
        let cfg: TopicBoundaryConfig = serde_json::from_str(
            r#"{
                "boundary_threshold": 1.7,
                "centroid_alpha": -0.4,
                "weight_centroid": 2.0,
                "weight_silence": -0.5,
                "weight_topic_length": 1.5
            }"#,
        )
        .expect("deserialize");
        assert!((cfg.boundary_threshold - 1.0).abs() < f32::EPSILON);
        assert!(cfg.centroid_alpha < f32::EPSILON);
        assert!((cfg.weight_centroid - 1.0).abs() < f32::EPSILON);
        assert!(cfg.weight_silence < f32::EPSILON);
        // weight_topic_length clamps to the unit interval then to < threshold.
        assert!(cfg.weight_topic_length < cfg.boundary_threshold);
    }

    #[test]
    fn topic_boundary_weight_topic_length_stays_below_threshold() {
        let cfg: TopicBoundaryConfig = serde_json::from_str(
            r#"{
                "boundary_threshold": 0.30,
                "weight_topic_length": 0.50
            }"#,
        )
        .expect("deserialize");
        assert!(
            cfg.weight_topic_length < cfg.boundary_threshold,
            "weight_topic_length={} must be < boundary_threshold={}",
            cfg.weight_topic_length,
            cfg.boundary_threshold
        );
        assert!(
            (cfg.weight_topic_length - 0.30f32.next_down().max(0.0)).abs() < f32::EPSILON,
            "expected clamp to next_down(threshold), got {}",
            cfg.weight_topic_length
        );
    }

    #[test]
    fn topic_boundary_non_finite_weights_become_zero() {
        // JSON cannot encode NaN/∞; exercise the unit-interval helper via a
        // dedicated visitor that feeds raw f32s through the same clamp path.
        assert!((clamp_unit_interval_f32(f32::NAN)).abs() < f32::EPSILON);
        assert!((clamp_unit_interval_f32(f32::INFINITY)).abs() < f32::EPSILON);
        assert!((clamp_unit_interval_f32(f32::NEG_INFINITY)).abs() < f32::EPSILON);
        assert!((clamp_unit_interval_f32(1.7) - 1.0).abs() < f32::EPSILON);
        assert!((clamp_unit_interval_f32(-0.2)).abs() < f32::EPSILON);
    }

    #[test]
    fn topic_boundary_round_trips_through_public_schema() {
        let cfg: TopicBoundaryConfig = serde_json::from_str(
            r#"{
                "enabled": false,
                "boundary_threshold": 0.7,
                "max_topic_turns": 20
            }"#,
        )
        .expect("deserialize");
        assert!(!cfg.enabled);
        assert!((cfg.boundary_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(cfg.max_topic_turns, 20);

        let json = serde_json::to_value(&cfg).expect("serialize");
        assert_eq!(json["enabled"], false);
        assert_eq!(json["max_topic_turns"], 20);
    }
}
