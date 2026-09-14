//! Steering and delegation orchestration (H-A): premise precheck, identity
//! minting, and repository outcome handling.

use ene_primitive::{RawId, WallClockWithTz};

use crate::agent::TaskAgentOutput;
use crate::context::{TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind};
use crate::delegation::{
    CreateDelegationCommand, DelegationCreationPremise, DelegationId, DelegationOutcome,
    TaskAgentEphemeralId,
};
use crate::repository::{
    ConversationTaskRepository, OwnerMessageCurrentness, TaskCommitOutcome, TaskRepository,
    TaskTechnicalError,
};
use crate::result::{TaskAgentResultArrival, TaskResultAcceptance, TaskResultId, TaskResultRecord};
use crate::task::{
    AssigneeRef, SteeringPremiseRef, TaskCommitPremise, TaskCreationOutcome, TaskCreationPremise,
    TaskId, TaskInstructionAdoptionPremise, TaskProgress, TaskPurpose, TaskPurposeAdoptionPremise,
    TaskRef,
};
use crate::workspace::{WorkspaceAssocId, WorkspaceAssociationPremise, WorkspaceNeedRef};

/// The Task-side premise for one Task proposal (H-A).
///
/// The caller maps its command-level request into this shape: it carries the
/// requester it proposes as the Task assignee, the proposed purpose text and
/// origin, and the workspace conditions. All identities
/// ([`TaskId`], [`TaskContextEntryId`], [`WorkspaceAssocId`]) and the
/// acquisition time are minted by [`orchestrate_task_creation`], never by the
/// caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProposalPremise {
    /// The Companion the caller proposes as the Task assignee.
    pub requester: AssigneeRef,
    /// The purpose text proposed for adoption.
    pub purpose: TaskPurpose,
    /// The record the purpose proposal came from.
    pub origin: TaskContextOrigin,
    /// The workspace conditions the proposal relies on; the owner confirms
    /// the association when they are present.
    pub workspace_need: Option<WorkspaceNeedRef>,
}

/// Orchestrates one Task creation proposal against the repository (AU2).
///
/// The orchestration mints the Task, its initial adopted-purpose context
/// entry, and — when the premise carries workspace conditions — the confirmed
/// workspace association identity, then writes the whole
/// [`TaskCreationPremise`] in one atomic commit. The requester becomes the
/// Task assignee. The returned outcome is [`TaskProposalOutcome::AcceptedAsTask`]
/// with the committed reference; the other [`TaskProposalOutcome`] variants
/// describe steering premises and cannot arise from a fresh creation, so a
/// creation never fabricates them. Repository technical errors stay `Err`.
pub async fn orchestrate_task_creation(
    repository: &impl TaskRepository,
    premise: TaskProposalPremise,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    let reference = repository
        .create_task(task_creation_premise(premise))
        .await?;
    Ok(TaskProposalOutcome::AcceptedAsTask(reference))
}

/// Orchestrates one conversation-sourced Task creation proposal.
///
/// Identical identity minting as [`orchestrate_task_creation`], but the commit
/// additionally requires the relied Owner input to still be the newest
/// accepted one: [`ConversationTaskRepository::create_task_from_conversation`]
/// compares that premise inside the same short transaction as the creation
/// unit, so a newer Owner input supersedes the turn and answers
/// [`TaskCreationOutcome::Superseded`] with zero writes.
pub async fn orchestrate_task_creation_current(
    repository: &impl ConversationTaskRepository,
    premise: TaskProposalPremise,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskCreationOutcome, TaskTechnicalError> {
    repository
        .create_task_from_conversation(task_creation_premise(premise), currentness)
        .await
}

/// Mints the AU2 creation unit from the proposal premise.
fn task_creation_premise(premise: TaskProposalPremise) -> TaskCreationPremise {
    let workspace = premise
        .workspace_need
        .map(|need| WorkspaceAssociationPremise {
            assoc: WorkspaceAssocId::generate(),
            need,
        });
    TaskCreationPremise {
        task: TaskId::generate(),
        purpose: premise.purpose,
        entry: TaskContextEntryId::generate(),
        origin: premise.origin,
        acquired_at: WallClockWithTz::now(),
        assignee: premise.requester,
        workspace,
    }
}

/// Re-evaluates one sealed result against the current canonical facts (AU15b
/// re-evaluation).
///
/// The producer only decides *that* one stored result needs a fresh adoption
/// judgement; the judgement itself stays [`TaskRepository::adopt_result`],
/// which re-enumerates the authoritative attempt set from the result's sealed
/// delegation, re-reads the current Action certainty, and re-checks the
/// current revision / purpose identity and the Task-wide completion barrier
/// inside its own short transaction. The claim passed here is re-derived from
/// durable facts by [`TaskRepository::load_result_adoption_claim`]; no old
/// blocker list is cached and no adoption state is duplicated. A missing
/// result row is the owner's `MissingResult` domain answer, and a missing
/// delegation is answered by the adoption commit itself. No provider call,
/// filesystem Action, or Task Agent run is triggered.
pub async fn reevaluate_result_adoption(
    repository: &impl TaskRepository,
    result: TaskResultId,
) -> Result<TaskResultAcceptance, TaskTechnicalError> {
    let Some(claim) = repository.load_result_adoption_claim(result).await? else {
        return Ok(TaskResultAcceptance::MissingResult { result });
    };
    repository.adopt_result(claim).await
}

/// The Task-side premise for one steering proposal (H-A).
///
/// The caller maps its command-level request into this shape: it carries only
/// the relied-on revision and purpose and the source record of the proposed
/// instruction. The adoption identities and the acquisition time are minted
/// by [`orchestrate_steering`], never by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteeringProposalPremise {
    /// The relied-on revision and purpose, compared against durable state.
    pub premise: SteeringPremiseRef,
    /// `Some` proposes a new purpose text; `None` keeps the current purpose.
    pub new_purpose: Option<TaskPurpose>,
    /// Reference to the utterance record proposing the instruction. This is an
    /// origin source, not the adoption identity.
    pub instruction_source: RawId,
}

/// The Task owner's domain result for one steering or creation proposal (H-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskProposalOutcome {
    /// A fresh Task was created exactly once ([`orchestrate_task_creation`]).
    AcceptedAsTask(TaskRef),
    /// The steering commit created exactly one new revision.
    AcceptedAsSteering(TaskRef),
    /// A newer accepted Owner input superseded the relied utterance; nothing
    /// was changed. Only the conversation-sourced guarded steering answers
    /// this.
    Superseded,
    /// The relied-on revision or purpose does not match the durable current
    /// state; nothing was changed and the caller re-evaluates.
    StalePremise { current: TaskRef },
    /// The Task is terminal (`Completed` / `Failed`); the revision and
    /// context are unchanged. Absorbing, so it is distinct from revision
    /// staleness.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    /// The premise names a Task with no durable state; nothing was changed.
    MissingTask { task: TaskId },
    /// No representable next revision can be durably committed; nothing was
    /// changed.
    RevisionExhausted { task: TaskId },
}

/// Orchestrates one steering proposal against the repository.
///
/// The precheck loads the durable current state and compares the caller's
/// `expected` revision and relied-on purpose identity against it; a mismatch
/// returns [`TaskProposalOutcome::StalePremise`] without minting or writing
/// anything. On a match the orchestration mints the adopted-purpose and
/// adopted-instruction entry identities and one acquisition time, and calls
/// [`TaskRepository::forward_steering`]. The precheck is not the concurrency
/// guarantee: `forward_steering` compares `expected` again inside its atomic
/// commit, so a competing winner between the precheck and the commit is still
/// mapped to `StalePremise`.
///
/// Outcome mapping: `CommittedAs` becomes `AcceptedAsSteering`,
/// `StaleExpected` becomes `StalePremise`, and `MissingTask` /
/// `RevisionExhausted` keep the same variant and payload. Repository technical
/// errors are returned as `Err` unchanged; domain outcomes are never folded
/// into them.
pub async fn orchestrate_steering(
    repository: &impl TaskRepository,
    proposal: SteeringProposalPremise,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    match prepare_steering(repository, &proposal).await? {
        SteeringPreparation::Refused(outcome) => Ok(outcome),
        SteeringPreparation::Ready(premise) => Ok(map_commit_outcome(
            repository.forward_steering(*premise).await?,
        )),
    }
}

/// Orchestrates one conversation-sourced steering proposal.
///
/// Identical to [`orchestrate_steering`] except that the commit additionally
/// requires the relied Owner input to still be the newest accepted one:
/// [`ConversationTaskRepository::forward_steering_from_conversation`] compares
/// that premise inside the same short transaction as the revision commit, so a
/// newer Owner input supersedes the turn and answers
/// [`TaskProposalOutcome::Superseded`] with zero writes.
pub async fn orchestrate_steering_current(
    repository: &impl ConversationTaskRepository,
    proposal: SteeringProposalPremise,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    match prepare_steering(repository, &proposal).await? {
        SteeringPreparation::Refused(outcome) => Ok(outcome),
        SteeringPreparation::Ready(premise) => {
            match repository
                .forward_steering_from_conversation(*premise, currentness)
                .await?
            {
                TaskCommitOutcome::Superseded => Ok(TaskProposalOutcome::Superseded),
                outcome => Ok(map_commit_outcome(outcome)),
            }
        }
    }
}

/// One prepared steering commit, or the owner's refusal.
enum SteeringPreparation {
    Ready(Box<TaskCommitPremise>),
    Refused(TaskProposalOutcome),
}

/// Runs the shared steering precheck and mints the commit premise.
async fn prepare_steering(
    repository: &impl TaskRepository,
    proposal: &SteeringProposalPremise,
) -> Result<SteeringPreparation, TaskTechnicalError> {
    let expected = proposal.premise.expected;
    let Some(record) = repository.load_task(expected.task).await? else {
        return Ok(SteeringPreparation::Refused(
            TaskProposalOutcome::MissingTask {
                task: expected.task,
            },
        ));
    };
    if record.task.reference != expected || record.task.purpose != proposal.premise.purpose {
        return Ok(SteeringPreparation::Refused(
            TaskProposalOutcome::StalePremise {
                current: record.task.reference,
            },
        ));
    }
    if record.task.progress.is_terminal() {
        return Ok(SteeringPreparation::Refused(
            TaskProposalOutcome::TaskTerminal {
                task: expected.task,
                progress: record.task.progress,
            },
        ));
    }
    let adopted_purpose_entry = TaskContextEntryId::generate();
    let entry = TaskContextEntryId::generate();
    let acquired_at = WallClockWithTz::now();
    let origin = TaskContextOrigin {
        kind: TaskContextOriginKind::OwnerConversation,
        source: proposal.instruction_source,
    };
    let new_purpose = proposal
        .new_purpose
        .clone()
        .map(|purpose| TaskPurposeAdoptionPremise {
            purpose,
            origin,
            acquired_at,
        });
    let adopted_instruction = Some(TaskInstructionAdoptionPremise {
        entry,
        origin,
        acquired_at,
    });
    Ok(SteeringPreparation::Ready(Box::new(TaskCommitPremise {
        expected,
        new_purpose,
        adopted_purpose_entry,
        adopted_instruction,
    })))
}

fn map_commit_outcome(outcome: TaskCommitOutcome) -> TaskProposalOutcome {
    match outcome {
        TaskCommitOutcome::CommittedAs(reference) => {
            TaskProposalOutcome::AcceptedAsSteering(reference)
        }
        TaskCommitOutcome::StaleExpected { current } => {
            TaskProposalOutcome::StalePremise { current }
        }
        // The unguarded commit cannot answer supersession; the guarded
        // mapping above consumes it before this mapper.
        TaskCommitOutcome::Superseded => TaskProposalOutcome::Superseded,
        TaskCommitOutcome::TaskTerminal { task, progress } => {
            TaskProposalOutcome::TaskTerminal { task, progress }
        }
        TaskCommitOutcome::MissingTask { task } => TaskProposalOutcome::MissingTask { task },
        TaskCommitOutcome::RevisionExhausted { task } => {
            TaskProposalOutcome::RevisionExhausted { task }
        }
    }
}

/// Orchestrates one delegation creation against the repository (H-A / AU3).
///
/// The precheck loads the durable current state: an absent Task returns
/// [`DelegationOutcome::MissingTask`], and a current revision different from
/// `command.task` returns [`DelegationOutcome::StaleTaskRevision`]; neither
/// path mints identities or writes anything. On a match the orchestration
/// mints the delegation and agent identities, builds the premise, and calls
/// [`TaskRepository::create_delegation`]. The precheck is not the
/// concurrency guarantee: `create_delegation` compares the revision again
/// inside its atomic commit, so a competing winner between the precheck and
/// the commit still yields [`DelegationOutcome::StaleTaskRevision`].
///
/// The repository outcome is passed through unchanged, and repository
/// technical errors stay `Err`; domain outcomes are never folded into them.
pub async fn orchestrate_delegation(
    repository: &impl TaskRepository,
    command: CreateDelegationCommand,
) -> Result<DelegationOutcome, TaskTechnicalError> {
    let Some(record) = repository.load_task(command.task.task).await? else {
        return Ok(DelegationOutcome::MissingTask {
            task: command.task.task,
        });
    };
    if record.task.reference != command.task {
        return Ok(DelegationOutcome::StaleTaskRevision {
            current: record.task.reference,
        });
    }
    if record.task.progress.is_terminal() {
        return Ok(DelegationOutcome::TaskTerminal {
            task: command.task.task,
            progress: record.task.progress,
        });
    }
    repository
        .create_delegation(DelegationCreationPremise {
            delegation: DelegationId::generate(),
            task: command.task,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: command.scope_copy,
        })
        .await
}

/// Orchestrates one explicit final result submission (AU15a).
///
/// This is the Task-owned finalization boundary the caller (in this stage the
/// Host or a test; later the Task Agent tool loop) invokes when it decides
/// that one delegated turn produced the **final** result. One
/// [`crate::TaskAgentTurnOutcome::Produced`] is only a provider output and may still
/// be an Action request or intermediate text: this function is never called
/// from that outcome automatically, and no provider output parsing or
/// automatic final-output detection exists here.
///
/// The orchestration mints the [`TaskResultId`], so the caller names no
/// identity, and records `{ delegation, result, body }` through
/// [`TaskRepository::record_task_result_arrival`] before returning. The
/// committed row is the execution seal; the arrival judges no currentness,
/// certainty, or completion. Repository technical errors (including a broken
/// delegation correspondence, a reused identity with different content, and a
/// second final result for the same delegation) stay `Err` and fail closed.
pub async fn orchestrate_result_arrival(
    repository: &impl TaskRepository,
    delegation: DelegationId,
    body: TaskAgentOutput,
) -> Result<TaskResultRecord, TaskTechnicalError> {
    repository
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation,
            result: TaskResultId::generate(),
            body,
        })
        .await
}
