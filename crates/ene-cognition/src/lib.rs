#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! # ene-cognition
//!
//! Cognitive runtime for the ene AI companion — memory extraction, recall planning,
//! emotion engine, context management, and prompt composition.
//!
//! ## Architecture
//!
//! The crate implements the [Ene Cognitive Runtime](../../docs/architecture/cognitive-runtime.md)
//! architecture, treating the LLM as an utterance generator from explicitly managed
//! cognitive state rather than as the entity that implicitly holds personality and memory.
//!
//! ## Crate Boundaries
//!
//! Enforced by the [Cognitive Runtime ADR](../../docs/architecture/cognitive-runtime.md)
//! and [AGENTS.md §4.1](../../AGENTS.md); see the
//! [API refactor plan](../../docs/architecture/api-refactor-plan.md) (item 2) for the
//! audit that reconfirmed these boundaries still hold.
//!
//! - Depends on: `ene-memory`, `ene-config`, `ene-provider`, `ene-common`
//! - Does NOT depend on: `ene-core`, `ene-session` (prevents circular dependencies)
//! - Calls `ene-memory` only through its public `MemoryStore` methods — never issues
//!   raw SQL or `sea-orm` queries directly. `ene-memory` remains the sole SQLite owner.
//! - Owns cognition logic exclusively: memory extraction, recall planning, emotion,
//!   context budgeting, and prompt composition all live here, not in `ene-core`.
//!   `ene-core` only *invokes* `CognitionEngine`; it must not reimplement cognition
//!   logic inline in its own streaming path.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_cognition::CognitionEngine;
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
/// Deterministic/LLM memory extraction and Memory Arbiter.
pub mod memory_writer;
/// Output arbitration: expression validation and hysteresis management.
pub mod output;
/// Pre-turn input analysis and turn intent classification.
pub mod pre_turn;
/// Sectioned prompt packet composition with budget-aware assembly.
pub mod prompt_packet;
/// Memory recall planning and hybrid search orchestration.
pub mod recall;

/// Cognitive runtime configuration section.
pub mod config;
/// Central cognitive engine facade.
pub mod engine;
/// Cognitive runtime error types.
pub mod error;
/// Turn lifecycle DTOs for streaming integration (#100).
pub mod lifecycle;

#[doc(no_inline)]
pub use commitments::{CommitmentLedger, CommitmentSyncContext};
/// Cognitive configuration section.
pub use config::{
    CharacterMemoryConfig, CognitionConfig, CognitionMemoryConfig, ContextConfig, EmotionConfig,
    EngineMode, ToolGroundingConfig,
};
/// Context budget and compression types.
#[doc(no_inline)]
pub use context::{
    ActiveSceneSummary, CompressionLevel, CompressionReason, CompressionResult,
    CompressionTaskInput, ContextBudget, ContextManager, MIN_MESSAGES_TO_COMPRESS, PackInput,
    PackedPrompt, PendingCompressionTask, compression_has_usable_summary,
    evaluate_compression_trigger, execute_compression, load_active_scene_summary,
    maybe_roll_up_chapter, pack_prompt, poll_compression_result, spawn_compression_task,
    validate_context_config,
};
/// Emotion engine types.
#[doc(no_inline)]
pub use emotion::{AffectProposal, EmotionEngine, TurnAffectInput};
/// Re-export commitment domain types from ene-memory for consumers.
#[doc(no_inline)]
pub use ene_memory::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
/// Central cognitive engine facade.
pub use engine::CognitionEngine;
/// Cognitive runtime error type.
pub use error::{CognitionError, EneCognitionError};
/// Turn lifecycle types for streaming integration.
pub use lifecycle::{
    ComposedPrompt, HistoryEntry, PostTurnInput, PreTurnOutput, PromptPacketMeta, TurnContext,
};
/// Memory arbiter and related decision types.
#[doc(no_inline)]
pub use memory_writer::{
    AppliedDecision, ArbiterAction, ArbiterContext, ArbiterOptions, ArbiterReason,
    ArbiterReasonCode, CandidateDecision, CandidateProvenance, ForgettingContext,
    ForgettingLifecycle, ForgettingReport, MemoryArbiter, SemanticMatch,
};
/// Expression arbiter types.
#[doc(no_inline)]
pub use output::{ExpressionDecision, ExpressionInput, ExpressionSource, OutputArbiter};
/// Prompt packet section types.
#[doc(no_inline)]
pub use prompt_packet::{PromptPacket, PromptSection, PromptSectionKind};
/// Recall planning types.
#[doc(no_inline)]
pub use recall::{
    EMOTIONAL_MATCH_REASON_THRESHOLD, LlmMemoryReranker, MemoryDiversifyOptions,
    MemoryDiversifyPipeline, MemoryRerankError, MemoryRerankInput, MemoryRerankOptions,
    MemoryRerankPipeline, MemoryReranker, PassthroughMemoryReranker, RecallBudgetHints, RecallPlan,
    RecallPlanner, RecallPlannerInput, RecallPlannerOptions, RecallReason, RecallResultMapper,
    RecallScopeFilter, RecallSearchHints, RecallTurn, RecalledMemory, explain_scored_memories,
    format_recalled_content, infer_recall_reason, recall_content_qualifier,
};
