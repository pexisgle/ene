//! The durable boundary for Task creation, steering, delegation, and reload.

use ene_primitive::RawId;
use thiserror::Error;

use crate::cancel::TaskCancelOutcome;
use crate::delegation::{
    DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
};
use crate::failure::{TaskFailureOutcome, TaskFailurePremise};
use crate::result::{
    TaskAgentResultArrival, TaskResultAcceptance, TaskResultAdoptionClaim, TaskResultId,
    TaskResultRecord, UnadoptedResultCursor,
};
use crate::task::{
    TaskCommitPremise, TaskCreationOutcome, TaskCreationPremise, TaskId, TaskProgress, TaskRecord,
    TaskRef,
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
    /// A newer accepted Owner input superseded the relied utterance; nothing
    /// was changed. Only the conversation-sourced guarded commit answers
    /// this.
    Superseded,
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

/// The Owner-utterance currentness premise of one conversation-sourced Task
/// control commit.
///
/// `message` is the committed Owner message record the directive's turn
/// relied on. The commit succeeds only while that record is still the newest
/// accepted Owner input for `companion`; a newer accepted input supersedes
/// the turn and the commit answers a `Superseded` outcome with zero writes.
/// The pair is comparison material for the store's single transaction, never
/// authority: the Task owner still performs its own revision, terminal, and
/// correspondence compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerMessageCurrentness {
    pub companion: RawId,
    pub message: RawId,
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

    /// Accepts or refuses one cancel request (AU16).
    ///
    /// One short `Immediate` transaction reads the current `task.progress` and
    /// moves `Started` / `InProgress` to
    /// [`Cancelled`](crate::TaskProgress::Cancelled) with a compare-and-set;
    /// that commit is the only durable fact of admission. The request carries
    /// no revision or purpose premise: cancel is Task-level and is never made
    /// stale by a concurrent steering (the steering forward stands, and the
    /// cancel is recorded on top of it).
    ///
    /// `Completed` / `Failed` return
    /// [`TaskTerminal`](crate::TaskCancelOutcome::TaskTerminal) and an already
    /// `Cancelled` Task returns
    /// [`AlreadyCancelled`](crate::TaskCancelOutcome::AlreadyCancelled), both
    /// with zero writes (admission happens exactly once). A missing identity
    /// returns [`MissingTask`](crate::TaskCancelOutcome::MissingTask). An
    /// unknown stored progress value and a CAS that does not move exactly one
    /// row are technical errors (fail closed), never a fabricated domain
    /// answer.
    ///
    /// No already-started activity is retracted: `inference_attempt`,
    /// `data_use`, `action_attempt`, and their certainty stay untouched, and
    /// no stop flag, stop row, or cancel-specific gate condition is written.
    /// Later admission attempts are refused by each boundary's existing
    /// non-terminal compare; already-started activity may still record its
    /// own facts (arrival/seal, verified correlation, certainty updates).
    async fn cancel_task(&self, task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError>;

    /// Commits one confirmed terminal failure (the `Failed` producer).
    ///
    /// One short `Immediate` transaction reads the current `task.progress`
    /// and moves `Started` / `InProgress` to
    /// [`Failed`](crate::TaskProgress::Failed) with a compare-and-set; that
    /// commit is the only durable fact of failure. The premise carries the
    /// relied `TaskRef` and, when the observation came from a delegated
    /// execution, its delegation. Before any write the commit verifies that
    /// the delegation exists, that it belongs to the premise's Task (a
    /// delegation of another Task is a fail-closed technical error), that its
    /// relied revision matches the premise, and that the current Task revision
    /// still equals the relied revision. `Completed` / `Cancelled` return
    /// [`TaskTerminal`](crate::TaskFailureOutcome::TaskTerminal) and an
    /// already `Failed` Task returns
    /// [`AlreadyFailed`](crate::TaskFailureOutcome::AlreadyFailed), both with
    /// zero writes. A stale revision returns
    /// [`StalePremise`](crate::TaskFailureOutcome::StalePremise) and a missing
    /// identity returns the matching `Missing*` variant. No already-started
    /// activity is retracted or re-executed: inference attempts, `data_use`,
    /// Action attempts, certainty, and result arrival stay untouched, and no
    /// failure-specific flag, row, or gate condition is written. `kind` is
    /// the caller's confirmed classification and is not persisted separately:
    /// the progress value is the single terminal truth.
    async fn fail_task(
        &self,
        premise: TaskFailurePremise,
    ) -> Result<TaskFailureOutcome, TaskTechnicalError>;

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

    /// Reports whether this delegated execution already started durable work.
    ///
    /// The first committed inference attempt (AU14) or Action start (AU5) of
    /// a delegation is its durable start marker: once one exists, that
    /// execution lifetime has begun and it is never started again under the
    /// same identity — a stopped unsealed run, a lost in-process running
    /// registration, and a restart are all covered by the same durable facts.
    /// Continued work is a new delegation. The probe reads the durable
    /// attempt facts and changes nothing; a missing delegation or Task is not
    /// an error here (the caller's own load answers those domain outcomes).
    async fn delegation_has_started_work(
        &self,
        delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError>;

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

    /// Re-derives the adoption claim of one stored result from durable facts.
    ///
    /// `None` means the result row does not exist. `Some(claim)` carries
    /// exactly the result-local Action attempt identities enumerated from the
    /// result's sealed delegation (execution lifetime), verified against the
    /// result's copied `(task, revision)` correspondence. The claim is
    /// comparison material for [`Self::adopt_result`], which re-enumerates
    /// the authoritative set itself; the producer never caches an old blocker
    /// list or previously stamped set. A missing delegation row yields the
    /// empty set here and is answered as
    /// [`MissingDelegation`](crate::TaskResultAcceptance::MissingDelegation)
    /// by the adoption commit.
    async fn load_result_adoption_claim(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError>;

    /// Lists sealed results that have not adopted yet and can still be
    /// re-adopted, starting strictly after `after`, in
    /// `(recorded_at, result_id)` order.
    ///
    /// The candidate predicate is canonical-facts only: `adopted_revision IS
    /// NULL` on an existing `task_result` row, the Task is non-terminal, and
    /// the Task's current revision equals the result's relied revision. A
    /// result that can only answer `RecordedToOriginalOnly` (cancelled /
    /// moved revision / terminal Task) is therefore not a candidate, so
    /// permanent history is not re-evaluated on every startup; no pending
    /// flag, queue state, or retry row is introduced. The cursor encodes the
    /// last candidate a previous page returned, so a bounded reconciliation
    /// can walk the whole candidate set without ever re-reading the prefix
    /// and without an unbounded `SELECT` or full in-memory materialization.
    /// The order is a storage-total order for traversal only, never
    /// currentness evidence. `limit` bounds the rows read by the storage
    /// query, not only the returned vector. The listing judges nothing: each
    /// candidate still goes through [`crate::reevaluate_result_adoption`],
    /// which re-evaluates against current facts and may legitimately stay
    /// withheld.
    async fn list_unadopted_results_after(
        &self,
        after: Option<UnadoptedResultCursor>,
        limit: u64,
    ) -> Result<Vec<UnadoptedResultCursor>, TaskTechnicalError>;

    /// Lists the Action attempt identities under one Task, in attempt-id
    /// order, for the report composition.
    ///
    /// The enumeration is the same Task-wide input the completion barrier
    /// reads: every revision and every delegation of the Task, from either the
    /// attempt's copied `task_id` or its delegation's correspondence, with
    /// each row's copied correlation verified. It is a read; it writes
    /// nothing, changes no certainty, and decides nothing. A Task with no
    /// attempts returns the empty set, and a caller-composed report may then
    /// load each attempt from its Action owner.
    async fn load_task_action_attempts(
        &self,
        task: TaskId,
    ) -> Result<Vec<RawId>, TaskTechnicalError>;
}

/// Conversation-sourced Task control commits.
///
/// These are the same Task owner operations as [`TaskRepository`], but the
/// store additionally requires the Owner message premise to still be current
/// inside the same short transaction as the Task write: a newer accepted
/// Owner input supersedes the dialogue turn, and the commit answers
/// `Superseded` with zero writes instead of acting on the past. They exist so
/// a superseded turn can never commit a Task side effect; the store
/// implements them on the one SQLite master that holds both the History and
/// the Task tables. First-party callers use the unguarded operations.
#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait ConversationTaskRepository: TaskRepository {
    /// Creates one Task iff the relied Owner input is still current.
    async fn create_task_from_conversation(
        &self,
        premise: TaskCreationPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCreationOutcome, TaskTechnicalError>;

    /// Commits one steering forward iff the relied Owner input is still
    /// current.
    async fn forward_steering_from_conversation(
        &self,
        premise: TaskCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError>;

    /// Accepts or refuses one cancel admission iff the relied Owner input is
    /// still current.
    async fn cancel_task_from_conversation(
        &self,
        task: TaskId,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError>;
}
