//! Recall planning for typed cognitive memory.
//!
//! This module produces query plans from the current turn context. It does not
//! execute database searches; callers pass the resulting [`RecallPlan`] to the
//! memory store or later recall pipeline stages.

mod input;
mod intent;
mod plan;
mod planner;
mod topic;

/// Recall planner input DTOs.
pub use input::{RecallPlannerInput, RecallTurn};
/// Recall plan output DTOs.
pub use plan::{RecallBudgetHints, RecallPlan, RecallScopeFilter, RecallSearchHints};
/// Recall planner implementation and options.
pub use planner::{RecallPlanner, RecallPlannerOptions};
