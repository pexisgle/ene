//! Recall planning for typed cognitive memory.
//!
//! This module produces query plans from the current turn context. It does not
//! execute database searches; callers pass the resulting [`RecallPlan`] to the
//! memory store or later recall pipeline stages.

mod executor;
mod input;
mod intent;
mod plan;
mod planner;
mod rerank;
mod result;
mod topic;

/// Cognition-side mapper from hybrid search output to explainable recall results.
pub use executor::RecallResultMapper;
/// Recall planner input DTOs.
pub use input::{RecallPlannerInput, RecallTurn};
/// Recall plan output DTOs.
pub use plan::{RecallBudgetHints, RecallPlan, RecallScopeFilter, RecallSearchHints};
/// Recall planner implementation and options.
pub use planner::{RecallPlanner, RecallPlannerOptions};
/// Optional memory reranking after hybrid search.
pub use rerank::{
    LlmMemoryReranker, MemoryRerankError, MemoryRerankInput, MemoryRerankOptions,
    MemoryRerankPipeline, MemoryReranker, PassthroughMemoryReranker,
};
/// Explainable recall result DTOs and helpers.
pub use result::{
    EMOTIONAL_MATCH_REASON_THRESHOLD, RecallReason, RecalledMemory, explain_scored_memories,
    infer_recall_reason,
};
