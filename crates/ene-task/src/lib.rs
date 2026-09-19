//! Task ownership: Task identity, purpose, context, workspace association,
//! and the repository boundary that persists them.
//!
//! A [`Task`] is one unit of tracked work. Its identity ([`TaskId`]) is
//! separate from the revision ([`TaskRevision`]) that orders the owner's
//! decisions; the two travel together as a [`TaskRef`]. The adopted purpose
//! is identified by [`TaskPurposeRef`] and its text lives in the revision
//! snapshot, so a purpose change is a revision forward and never a change of
//! another lifecycle's generation.
//!
//! This crate owns the semantics and the [`TaskRepository`] contract; the
//! persistent implementation lives behind that trait (see `ene-store`), so
//! this crate never depends on the store, and it never imports another
//! domain's newtype: cross-domain identities arrive as owner-defined
//! premises.

mod agent;
mod cancel;
mod context;
mod delegation;
mod failure;
mod instruction;
mod observation;
mod orchestrate;
mod report;
mod repository;
mod result;
mod resume;
mod task;
mod workspace;

pub use agent::{
    TaskAgentActionExchange, TaskAgentInference, TaskAgentInferenceError,
    TaskAgentInferenceOutcome, TaskAgentInferencePremise, TaskAgentInferenceProduced,
    TaskAgentNotSent, TaskAgentObservation, TaskAgentOutput, TaskAgentTurnError,
    TaskAgentTurnOutcome, TaskAgentTurnPremise, orchestrate_task_agent_turn,
};
pub use cancel::{CancelTaskCommand, TaskCancelOutcome};
pub use context::{
    TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin, TaskContextOriginKind,
};
pub use delegation::{
    CreateDelegationCommand, DelegatedWorkspace, DelegationCreationPremise, DelegationId,
    DelegationOutcome, DelegationRef, DelegationScope, TaskAgentEphemeralId,
};
pub use failure::{TaskFailureKind, TaskFailureOutcome, TaskFailurePremise};
pub use instruction::{
    TaskInstructionRole, TaskInstructionSource, TaskInstructionSourceError,
    TaskInstructionSourceRecord,
};
pub use observation::{TaskAgentObservationId, TaskAgentObservationPremise};
pub use orchestrate::{
    SteeringProposalPremise, TaskProposalOutcome, TaskProposalPremise, orchestrate_delegation,
    orchestrate_result_arrival, orchestrate_steering, orchestrate_steering_current,
    orchestrate_task_creation, orchestrate_task_creation_current, reevaluate_result_adoption,
};
pub use report::{
    PAST_FACTS_ENTRY_CAP, PastExecutedFact, PastExecutedFactsPage, REPORT_PAGE_MAX, TaskHeadline,
    TaskReportRow, TaskReportRowCursor, TaskReportRowKind, TaskReportSourcePage,
    TaskReportSourceRef,
};
pub use repository::{
    ConversationTaskRepository, OwnerMessageCurrentness, TaskCommitOutcome, TaskRepository,
    TaskTechnicalError,
};
pub use result::{
    TaskAgentResultArrival, TaskResultAcceptance, TaskResultAdoptionClaim,
    TaskResultArrivalOutcome, TaskResultId, TaskResultRecord, TaskResultScrubPremise,
    UnadoptedResultCursor,
};
pub use resume::{
    ResumeInstructionSource, ResumeTaskCommand, TaskResumeCommitPremise, TaskResumeHold,
    TaskResumeOutcome, TaskResumeReadiness, orchestrate_resume, orchestrate_resume_current,
    resume_commit_premise, route_available_result,
};
pub use task::{
    AssigneeRef, SteeringPremiseRef, Task, TaskCommitPremise, TaskCreationOutcome,
    TaskCreationPremise, TaskId, TaskInstructionAdoptionPremise, TaskProgress, TaskPurpose,
    TaskPurposeAdoptionPremise, TaskPurposeRef, TaskRecord, TaskRef, TaskRevision,
    TaskRevisionRecord,
};
pub use workspace::{
    WorkspaceAssocId, WorkspaceAssociation, WorkspaceAssociationPremise, WorkspaceFolderRef,
    WorkspaceNeedRef,
};
