//! The durable boundary for Task creation, steering, and reload.

use thiserror::Error;

use crate::task::{TaskCommitPremise, TaskCreationPremise, TaskId, TaskRecord, TaskRef};

/// Technical failure for Task persistence.
///
/// Domain acceptance is never this error; stale, missing, and exhausted
/// commits are [`TaskCommitOutcome`] on the `Ok` side, and missing identities
/// are [`None`] on the `Ok` side of reads. Integrity failures that a forward
/// must not normalize are reported here.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskTechnicalError {
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause. Never Task content.
        reason: String,
    },
}

/// The domain result of one steering commit (AU4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCommitOutcome {
    /// The commit created exactly one new revision.
    CommittedAs(TaskRef),
    /// The expected revision no longer matches; nothing was changed.
    StaleExpected { current: TaskRef },
    /// The premise names a Task with no durable state; nothing was changed.
    MissingTask { task: TaskId },
    /// No representable next revision exists; nothing was changed.
    RevisionExhausted { task: TaskId },
}

/// Durable Task boundary.
///
/// [`Self::create_task`] writes the current Task row, its initial revision
/// record, the initial context entries, and a confirmed workspace
/// association in one atomic section: the commit is the only visibility
/// boundary, and a failed insert leaves no partial row behind.
#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait TaskRepository: Send + Sync {
    /// Creates one Task at its initial revision and returns its reference.
    ///
    /// The caller mints the identities in the premise. `workspace` is
    /// `None` when the Task has no confirmed workspace association.
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError>;

    /// Commits one steering forward at exactly `premise.expected.revision`.
    ///
    /// The expected revision is compared against the current row inside the
    /// commit, so concurrent steering serializes: the winner advances by one
    /// revision and the loser returns [`TaskCommitOutcome::StaleExpected`]
    /// without changing anything. A missing Task returns
    /// [`TaskCommitOutcome::MissingTask`] with no change. A successful commit
    /// writes the new revision snapshot, the new revision's adopted-purpose
    /// context entry, the adopted-instruction context entry when
    /// `premise.adopted_instruction` is `Some`, and the current pointer
    /// atomically; older revisions and context entries are retained.
    ///
    /// The caller mints the new revision's context entry identities; the
    /// repository persists them and stamps only the post-CAS `(task,
    /// revision)` reference and the adopted revision. An adopted-instruction
    /// entry is written once and is not re-recorded by a later forward.
    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError>;

    /// Loads the committed current unit of one Task: the current revision's
    /// snapshot and adopted-purpose entry, plus every adopted-instruction
    /// entry in force (each entry keeps its own adoption reference).
    ///
    /// `None` means the identity has no stored Task. Partial or inconsistent
    /// rows, unknown context item kinds, kind/payload mismatches, and entries
    /// beyond the current revision are never composed into a [`TaskRecord`];
    /// that is a technical error.
    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError>;
}
