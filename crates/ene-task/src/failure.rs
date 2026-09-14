//! Explicit confirmed terminal Task failure.
//!
//! A Task becomes [`TaskProgress::Failed`](crate::TaskProgress::Failed) only
//! when the Task owner commits a confirmed terminal failure: the work side
//! determined that the current purpose cannot be achieved and that no retry,
//! new delegation, or user clarification resolves it. Low-level trouble is
//! never Task failure, and the closed-world [`TaskFailureKind`] has no variant
//! for it: provider transient failures, `NotSent`, consent / credential /
//! deletion refusals, `ActionCertainty::Unknown`, withheld result adoption,
//! cancel, malformed provider output, turn-bound stops, and process crashes
//! keep their own typed outcomes and durable facts.
//!
//! The commit is one compare-and-set on the single `task.progress` master.
//! There is no failure shadow table, gate, or lifecycle: `Failed` is a
//! terminal value of the same progress the cancel and completion producers
//! write, and every existing admission gate already refuses new work for it.
//! The premise carries the relied-on Task revision and, when the work-side
//! observation came from a delegated execution, that delegation, so a stale
//! worker can never terminate a Task that has moved past its premise.

use crate::delegation::DelegationId;
use crate::task::{TaskId, TaskProgress, TaskRef};

/// One confirmed terminal-failure classification, closed world.
///
/// The vocabulary starts with exactly the outcome the Task owner can confirm
/// today; transient, uncertain, cancelled, or technical outcomes have no
/// variant by construction, so they can never be phrased as a Task failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskFailureKind {
    /// The work side confirmed that the current purpose cannot be achieved
    /// for this Task: retrying, creating another delegation, or asking the
    /// Owner for clarification does not resolve it.
    ConfirmedUnachievable,
}

/// The premise for one confirmed terminal-failure commit.
///
/// `task` is the relied-on revision (comparison material, not authority) and
/// `delegation` is the delegated execution the failure observation came from,
/// when one exists. The owner verifies the delegation correspondence inside
/// the commit; a delegation belonging to another Task is a fail-closed
/// technical error, and a delegation whose relied revision disagrees with
/// `task` is a stale domain outcome with zero writes. `kind` is the caller's
/// confirmed classification, never inferred by the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskFailurePremise {
    pub task: TaskRef,
    pub delegation: Option<DelegationId>,
    pub kind: TaskFailureKind,
}

/// The Task owner's domain result for one failure request.
///
/// Every variant is an `Ok`-side domain answer; the commit applies exactly
/// once and a repeated request is idempotent. `FailedAs` means only that the
/// current non-terminal progress moved to `Failed`; it claims nothing about
/// any external effect or already-started activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFailureOutcome {
    /// The current non-terminal progress (`Started` / `InProgress`) was moved
    /// to `Failed` in one atomic compare-and-set.
    FailedAs(TaskRef),
    /// The Task is already `Failed`; the idempotent re-request wrote nothing.
    AlreadyFailed { task: TaskId },
    /// The Task is terminal for another reason (`Completed` / `Cancelled`)
    /// and cannot be failed; nothing was written.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    /// The relied-on Task revision no longer matches the current revision
    /// (or the delegation was created against a different revision);
    /// nothing was written.
    StalePremise { current: TaskRef },
    /// The premise names a Task with no durable state; nothing was written.
    MissingTask { task: TaskId },
    /// The premise names a delegation that does not exist; nothing was
    /// written.
    MissingDelegation { delegation: DelegationId },
}
