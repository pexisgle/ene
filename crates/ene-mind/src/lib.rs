#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit/integration tests use unwrap/expect/panic for concise assertions"
    )
)]

//! # ene-mind
//!
//! Mind runtime for the ene AI companion — session state, memory extraction,
//! recall planning, emotion engine, context management, and prompt composition.
//!
//! ## Architecture
//!
//! The crate implements the [Ene Cognitive Runtime](../../docs/reference/architecture/cognitive-runtime.md)
//! architecture, treating the LLM as an utterance generator from explicitly managed
//! cognitive state rather than as the entity that implicitly holds personality and memory.
//! Conversation session state absorbed from the former standalone session crate
//! lives under [`session`].
//!
//! ## Crate Boundaries
//!
//! Enforced by the [Cognitive Runtime ADR](../../docs/reference/architecture/cognitive-runtime.md)
//! and the architecture boundaries; see the
//! [API v1 ADR](../../docs/reference/architecture/api-v1.md) for the target crate map.
//!
//! - Depends on: `ene-core`, `ene-config`, `ene-ai`
//! - Does NOT depend on: `ene-runtime` (prevents circular dependencies),
//!   `ene-store` (production code uses `ene_core::MemoryPort`; `ene-store`
//!   is a dev-dependency for integration tests only, #309)
//! - Calls the store only through `ene_core::MemoryPort` — never issues
//!   raw SQL or `sea-orm` queries directly. `ene-store` remains the sole `SQLite` owner.
//! - Owns mind logic exclusively: memory extraction, recall planning, emotion,
//!   context budgeting, prompt composition, and session state all live here.
//!   `ene-runtime` only *invokes* [`CognitionEngine`]; it must not reimplement mind
//!   logic inline in its own streaming path.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_mind::CognitionEngine;
//!
//! let engine = CognitionEngine::new();
//! ```

/// Character processor: Identity Kernel compilation and lorebook indexing.
pub mod character;
/// Companion Commitment Ledger: promise, task, and follow-up tracking.
pub mod commitments;
/// Context budget management and rolling compression.
pub mod context;
/// Emotion Engine: deterministic + optional LLM affect computation.
pub mod emotion;
/// Diagnostics/CLI memory journal search facade (#123).
pub mod memory_journal;
/// Deterministic/LLM memory extraction and Memory Arbiter.
pub mod memory_writer;
/// Output arbitration: expression validation and hysteresis management.
pub mod output;
/// Proactive companion speech decision pipeline (#103).
pub mod proactive;
/// Sectioned prompt packet composition with budget-aware assembly.
pub mod prompt_packet;
/// Memory recall planning and hybrid search orchestration.
pub mod recall;
/// Conversation session state, splitting, and performance-marker parsing.
pub mod session;
/// LLM-driven conversation summarization for session boundaries.
pub mod summarizer;

/// Mind runtime configuration section.
pub mod config;
/// Central cognitive engine facade.
pub mod engine;
/// Cognitive runtime error types.
pub mod error;
/// Turn lifecycle DTOs for streaming integration (#100).
pub mod lifecycle;

#[doc(no_inline)]
pub use commitments::{CommitmentLedger, CommitmentSyncContext};
/// Mind configuration section.
pub use config::{
    CharacterMemoryConfig, ContextConfig, EmotionConfig, MindConfig, MindMemoryConfig,
    ProactiveConfig, ProactiveDecisionConfig, ProactiveSourcesConfig, SessionConfig,
    ToolGroundingConfig, TopicBoundaryConfig, WindowTitleLevel,
};
/// Context budget and compression types.
#[doc(no_inline)]
pub use context::{
    ActiveSceneSummary, CompressionLevel, CompressionReason, CompressionResult,
    CompressionTaskInput, ContextBudget, ContextManager, MIN_MESSAGES_TO_COMPRESS, PackInput,
    PackedPrompt, PendingCompressionTask, RetroactiveCompressionPlan,
    compression_has_usable_summary, estimate_history_tokens, evaluate_compression_trigger,
    execute_compression, load_active_scene_summary, maybe_roll_up_chapter, pack_prompt,
    plan_retroactive_compression, poll_compression_result, spawn_compression_task,
};
/// Emotion engine types.
#[doc(no_inline)]
pub use emotion::{AffectProposal, EmotionEngine, TurnAffectInput};
/// Re-export commitment domain types from ene-core for consumers (#309).
#[doc(no_inline)]
pub use ene_core::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
/// Central cognitive engine facade.
pub use engine::{CognitionEngine, MemoryWriteOutcome};
/// Cognitive runtime error type.
pub use error::{CognitionError, EneCognitionError, MindError};
/// Turn lifecycle types for streaming integration.
pub use lifecycle::{
    ComposePrefetch, ComposedPrompt, HistoryEntry, OwnedPostTurnInput, OwnedTurnInput,
    PostTurnInput, PreTurnOutput, PromptPacketMeta, TurnContext, interruption_note,
};
/// Journal-style scored memory search (#123).
#[doc(no_inline)]
pub use memory_journal::MemoryJournal;
/// Memory arbiter and related decision types.
#[doc(no_inline)]
pub use memory_writer::{
    AppliedDecision, ArbiterAction, ArbiterContext, ArbiterOptions, ArbiterReason,
    ArbiterReasonCode, CandidateDecision, CandidateProvenance, ForgettingContext,
    ForgettingLifecycle, ForgettingReport, MemoryArbiter, MemoryWriteProviders, SemanticMatch,
};
/// Performance cue types for chat presentation (#126, #128).
#[doc(no_inline)]
pub use output::{CueSource, MotionLayer, PerfKind, PerformanceCue, cue_source_priority};
/// Expression arbiter types.
#[doc(no_inline)]
pub use output::{
    ExpressionDecision, ExpressionInput, ExpressionSource, OutputArbiter, PerformanceArbiter,
};
/// Proactive companion speech decision types (#103).
#[doc(no_inline)]
pub use proactive::{
    ActivitySnapshot, GateRejectReason, ProactiveContext, ProactiveDecision,
    ProactiveDecisionOutcome, ProactiveObservation, ProactiveSkipReason, ProactiveSuppressionState,
    ProactiveUrgency, ScreenSummaryStatus, build_decision_messages, build_proactive_context,
    decide_proactive_speech, decision_schema, evaluate_deterministic_gates, parse_decision_json,
};
/// Prompt packet section types.
#[doc(no_inline)]
pub use prompt_packet::{PromptPacket, PromptSection, PromptSectionKind};
/// Recall planning types.
#[doc(no_inline)]
pub use recall::{
    EMOTIONAL_MATCH_REASON_THRESHOLD, MemoryDiversifyOptions, MemoryDiversifyPipeline,
    RecallBudgetHints, RecallPlan, RecallPlanner, RecallPlannerInput, RecallPlannerOptions,
    RecallReason, RecallResultMapper, RecallScopeFilter, RecallSearchHints, RecallTurn,
    RecalledMemory, explain_scored_memories, format_recalled_content, infer_recall_reason,
    recall_content_qualifier,
};
/// Session types absorbed from the former standalone session crate.
#[doc(no_inline)]
pub use session::{
    CardName, CharacterAsset, CharacterCardData, CharacterCardV3, ConversationSession,
    EneSessionError, ExpressionDefinition, InterruptedState, ResolvedExpression, Role, SessionId,
    SplitReason, SplitResult, TopicBoundarySignal, TopicBoundaryTracker, Truncate,
    expand_cbs_macros, generate_session_id, parse_performance_marker, resolve_expressions,
    split_text_and_special_tokens, strip_markers,
};
/// Conversation summary result and LLM summarization entry point.
#[doc(no_inline)]
pub use summarizer::{ConversationSummaryResult, summarize_conversation};

/// Returns `true` if `haystack` contains any of `needles` as a substring.
pub(crate) fn contains_any<I, S>(haystack: &str, needles: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    needles
        .into_iter()
        .any(|needle| haystack.contains(needle.as_ref()))
}
