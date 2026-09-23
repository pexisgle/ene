use ene_primitive::RawId;
use thiserror::Error;

use crate::cancel::TaskCancelOutcome;
use crate::delegation::{
    DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
};
use crate::failure::{TaskFailureOutcome, TaskFailurePremise};
use crate::observation::{TaskAgentObservationId, TaskAgentObservationPremise};
use crate::orchestrate::TaskProposalOutcome;
use crate::report::{
    PastExecutedFactsPage, TaskHeadline, TaskReportRow, TaskReportRowCursor, TaskReportSourcePage,
    TaskReportSourceRef,
};
use crate::result::{
    TaskAgentResultArrival, TaskResultAcceptance, TaskResultAdoptionClaim,
    TaskResultArrivalOutcome, TaskResultId, TaskResultRecord, UnadoptedResultCursor,
};
use crate::resume::{TaskResumeCommitPremise, TaskResumeOutcome};
use crate::task::{
    TaskCommitPremise, TaskCreationPremise, TaskId, TaskProgress, TaskRecord, TaskRef,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskTechnicalError {
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCommitOutcome {
    CommittedAs(TaskRef),
    StaleExpected {
        current: TaskRef,
    },
    Superseded,
    /// The Task is terminal (`Completed` / `Failed` / `Cancelled`); the revision and the
    /// context are unchanged. Absorbing, so it is distinct from revision
    /// staleness.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    MissingTask {
        task: TaskId,
    },
    RevisionExhausted {
        task: TaskId,
    },
    HeldForErasure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerMessageCurrentness {
    pub companion: RawId,
    pub message: RawId,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait TaskRepository: Send + Sync {
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError>;

    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError>;

    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError>;

    async fn cancel_task(&self, task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError>;

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
    /// disagreeing snapshot is a technical error, never a composed value). The
    /// same atomic snapshot reads the current progress: a missing Task returns
    /// [`DelegationOutcome::MissingTask`], terminal progress (`Completed`,
    /// `Failed`, or `Cancelled`) returns [`DelegationOutcome::TaskTerminal`],
    /// and a revision mismatch on a non-terminal Task returns
    /// [`DelegationOutcome::StaleTaskRevision`], all `Ok`-side domain outcomes
    /// with zero writes. Terminal is decided before the revision compare, so it
    /// is never folded into a stale answer.
    ///
    /// Creation never advances the Task revision and imposes no single
    /// delegation constraint: multiple delegations of the same Task revision
    /// are valid, and each is a new identity.
    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError>;

    async fn load_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError>;

    async fn record_task_result_arrival(
        &self,
        arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultArrivalOutcome, TaskTechnicalError>;

    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError>;

    async fn load_delegation_result(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError>;

    async fn delegation_has_started_work(
        &self,
        delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError>;

    async fn record_task_agent_observation(
        &self,
        premise: TaskAgentObservationPremise,
    ) -> Result<TaskAgentObservationId, TaskTechnicalError>;

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError>;

    async fn load_result_adoption_claim(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError>;

    async fn list_unadopted_results_after(
        &self,
        after: Option<UnadoptedResultCursor>,
        limit: u64,
    ) -> Result<Vec<UnadoptedResultCursor>, TaskTechnicalError>;

    async fn load_task_action_attempts(
        &self,
        task: TaskId,
    ) -> Result<Vec<RawId>, TaskTechnicalError>;

    /// Lists one bounded page of Task lifecycle headlines in canonical
    /// `TaskId` byte order, starting strictly after `after`.
    ///
    /// `limit` is clamped to `1..=REPORT_PAGE_MAX` and applied by the SQL
    /// query, so the bound is on the rows read. Every headline is stored
    /// facts only — current revision, progress, and purpose — never a body.
    /// The read runs no reconciliation, starts no runner, and re-evaluates no
    /// stored result: "currently executing" is Host memory, so a
    /// non-terminal Task with no registration is reported as saved and not
    /// running.
    async fn list_tasks_after(
        &self,
        after: Option<TaskId>,
        limit: u32,
    ) -> Result<Vec<TaskHeadline>, TaskTechnicalError>;

    async fn list_task_report_rows_after(
        &self,
        task: TaskId,
        after: Option<TaskReportRowCursor>,
        limit: u32,
    ) -> Result<Vec<TaskReportRow>, TaskTechnicalError>;

    async fn load_report_source_bounded(
        &self,
        source: TaskReportSourceRef,
        cursor_bytes: u64,
        limit_bytes: u32,
    ) -> Result<Option<TaskReportSourcePage>, TaskTechnicalError>;

    async fn commit_task_resume(
        &self,
        premise: TaskResumeCommitPremise,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError>;

    async fn load_past_executed_facts(
        &self,
        task: TaskId,
    ) -> Result<PastExecutedFactsPage, TaskTechnicalError>;
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait ConversationTaskRepository: TaskRepository {
    async fn create_task_from_conversation(
        &self,
        premise: TaskCreationPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskProposalOutcome, TaskTechnicalError>;

    async fn forward_steering_from_conversation(
        &self,
        premise: TaskCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError>;

    async fn cancel_task_from_conversation(
        &self,
        task: TaskId,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError>;

    async fn commit_task_resume_from_conversation(
        &self,
        premise: TaskResumeCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError>;
}
