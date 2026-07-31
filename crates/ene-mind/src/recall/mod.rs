//! Recall planning for typed cognitive memory.
//!
//! This module produces query plans from the current turn context. It does not
//! execute database searches; callers pass the resulting [`RecallPlan`] to the
//! memory store or later recall pipeline stages.

mod diversify;
mod executor;
mod input;
mod intent;
mod lorebook_boost;
mod plan;
mod planner;
mod prompt_qualifier;
mod result;
mod runner;
mod topic;

/// MMR diversification after hybrid search (#78).
pub use diversify::{MemoryDiversifyOptions, MemoryDiversifyPipeline};
/// Cognition-side mapper from hybrid search output to explainable recall results.
pub use executor::RecallResultMapper;
/// Recall planner input DTOs.
pub use input::{RecallPlannerInput, RecallTurn};
/// Lorebook constant/key recall merge (#83).
pub use lorebook_boost::merge_lorebook_recall;
/// Recall plan output DTOs.
pub use plan::{RecallBudgetHints, RecallPlan, RecallScopeFilter, RecallSearchHints};
/// Recall planner implementation and options.
pub use planner::{RecallPlanner, RecallPlannerOptions};
/// Prompt qualifiers for recalled memory content (#76).
pub use prompt_qualifier::{format_recalled_content, recall_content_qualifier};
/// Explainable recall result DTOs and helpers.
pub use result::{
    EMOTIONAL_MATCH_REASON_THRESHOLD, RecallReason, RecalledMemory, explain_scored_memories,
    infer_recall_reason,
};
/// Hybrid recall execution for streaming integration (#100).
pub use runner::{ExecuteRecallInput, bump_injected_memory_access, execute_hybrid_recall};
