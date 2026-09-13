//! The durable boundary for Task creation, steering, delegation, and reload.

use thiserror::Error;

use crate::delegation::{
    DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
};
use crate::result::{
    TaskAgentResultArrival, TaskResultAcceptance, TaskResultAdoptionClaim, TaskResultId,
    TaskResultRecord,
};
use crate::task::{
    TaskCommitPremise, TaskCreationPremise, TaskId, TaskProgress, TaskRecord, TaskRef,
};

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
    /// The Task is terminal (`Completed` / `Failed`); the revision and the
    /// context are unchanged. Absorbing, so it is distinct from revision
    /// staleness.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
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

    /// Creates one delegation correspondence for a Task revision (AU3).
    ///
    /// [`orchestrate_delegation`](crate::orchestrate_delegation) mints the
    /// delegation and agent identities and passes them in the premise; the
    /// repository never re-allocates them. Inside the atomic compare the
    /// current Task row is read at exactly `premise.task.revision`, and the
    /// assignee copied into the delegation is that row's assignee, checked
    /// against the same revision's `task_revision` snapshot (a missing or
    /// disagreeing snapshot is a technical error, never a composed value). A
    /// missing Task returns [`DelegationOutcome::MissingTask`] and a revision
    /// mismatch returns [`DelegationOutcome::StaleTaskRevision`], both
    /// `Ok`-side domain outcomes with zero writes.
    ///
    /// Creation never advances the Task revision and imposes no single
    /// delegation constraint: multiple delegations of the same Task revision
    /// are valid, and each is a new identity.
    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError>;

    /// Loads the correspondence of one delegation.
    ///
    /// `None` means the identity has no stored delegation. Malformed or
    /// impossible rows are a technical error and are never composed into a
    /// [`DelegationRef`]. Row existence is not liveness: the ephemeral agent
    /// behind an existing row may already be gone, and this read infers no
    /// delegation lifecycle state.
    async fn load_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError>;

    /// Records one final result arrival and seals its delegation (AU15a).
    ///
    /// Called by orchestration at the explicit finalization boundary before
    /// the result becomes visible. The relied `(task, revision)` is copied
    /// from the delegation row inside one short `Immediate` transaction; the
    /// committed row's existence is the execution seal, so no separate seal
    /// state exists. A retry of the same [`TaskResultId`] is idempotent when
    /// the body, delegation, and relied revision match exactly; a different
    /// body or relied revision under the same identity, and a second final
    /// result for an already-sealed delegation, are technical errors (fail
    /// closed, never a domain outcome). This step judges no currentness,
    /// certainty, terminal state, or completion.
    async fn record_task_result_arrival(
        &self,
        arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultRecord, TaskTechnicalError>;

    /// Loads one final result by identity, with its verified attempt
    /// correlation and adoption state.
    ///
    /// `None` means the identity has no stored result. Malformed rows and
    /// inconsistent correlation are technical errors and are never composed
    /// into a [`TaskResultRecord`].
    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError>;

    /// Loads the final result of one delegation, if its execution is sealed.
    ///
    /// This is the bounded seal read: `Some` means the delegation already
    /// submitted a final result and admits no new inference claim (AU14) or
    /// Action start (AU5). `None` means no final result was recorded; it is
    /// not evidence about liveness.
    async fn load_delegation_result(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError>;

    /// Attempts to adopt one recorded final result into its Task (AU15b).
    ///
    /// Inside one short `Immediate` transaction: the result row is read, the
    /// authoritative Action attempt set is enumerated from its delegation
    /// (execution lifetime) with each row's `(delegation, task, revision)`
    /// correspondence verified, the claim must match that set exactly
    /// (missing, extra, and duplicate refs are technical errors), the current
    /// revision and purpose identity are compared against the relied
    /// revision's snapshot, terminal progress yields
    /// [`TaskResultAcceptance::RecordedToOriginalOnly`], and the Task-wide
    /// completion barrier (no `Unknown` attempt under the same `TaskId`,
    /// across every revision and delegation) is evaluated. Blockers record
    /// the result-local correlation and return
    /// [`TaskResultAcceptance::WithheldByEffectFacts`]; an empty blocker set
    /// stamps the correlation, marks `adopted_revision`, and CASes the
    /// progress to `Completed` in the same transaction. The Task owner never
    /// changes Action certainty.
    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError>;
}
