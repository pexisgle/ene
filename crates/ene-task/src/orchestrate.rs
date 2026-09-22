use ene_primitive::{RawId, WallClockWithTz};

use crate::context::{TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind};
use crate::delegation::{
    CreateDelegationCommand, DelegationCreationPremise, DelegationId, DelegationOutcome,
    TaskAgentEphemeralId,
};
use crate::repository::{
    ConversationTaskRepository, OwnerMessageCurrentness, TaskCommitOutcome, TaskRepository,
    TaskTechnicalError,
};
use crate::result::{
    TaskAgentResultArrival, TaskResultAcceptance, TaskResultArrivalOutcome, TaskResultId,
    TaskResultScrubPremise,
};
use crate::task::{
    AssigneeRef, SteeringPremiseRef, TaskCommitPremise, TaskCreationOutcome, TaskCreationPremise,
    TaskId, TaskInstructionAdoptionPremise, TaskProgress, TaskPurpose, TaskPurposeAdoptionPremise,
    TaskRef,
};
use crate::workspace::{WorkspaceAssocId, WorkspaceAssociationPremise, WorkspaceNeedRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProposalPremise {
    pub requester: AssigneeRef,
    pub purpose: TaskPurpose,
    pub origin: TaskContextOrigin,
    pub workspace_need: Option<WorkspaceNeedRef>,
}

pub async fn orchestrate_task_creation(
    repository: &impl TaskRepository,
    premise: TaskProposalPremise,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    let reference = repository
        .create_task(task_creation_premise(premise))
        .await?;
    Ok(TaskProposalOutcome::AcceptedAsTask(reference))
}

pub async fn orchestrate_task_creation_current(
    repository: &impl ConversationTaskRepository,
    premise: TaskProposalPremise,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskCreationOutcome, TaskTechnicalError> {
    repository
        .create_task_from_conversation(task_creation_premise(premise), currentness)
        .await
}

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

pub async fn reevaluate_result_adoption(
    repository: &impl TaskRepository,
    result: TaskResultId,
) -> Result<TaskResultAcceptance, TaskTechnicalError> {
    let Some(claim) = repository.load_result_adoption_claim(result).await? else {
        return Ok(TaskResultAcceptance::MissingResult { result });
    };
    repository.adopt_result(claim).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteeringProposalPremise {
    pub premise: SteeringPremiseRef,
    pub new_purpose: Option<TaskPurpose>,
    pub instruction_source: RawId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskProposalOutcome {
    AcceptedAsTask(TaskRef),
    AcceptedAsSteering(TaskRef),
    Superseded,
    /// The relied-on revision or purpose does not match the durable current
    /// state; nothing was changed and the caller re-evaluates.
    StalePremise { current: TaskRef },
    /// The Task is terminal (`Completed` / `Failed` / `Cancelled`); the revision and
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

enum SteeringPreparation {
    Ready(Box<TaskCommitPremise>),
    Refused(TaskProposalOutcome),
}

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
    if record.task.progress.is_terminal() {
        return Ok(SteeringPreparation::Refused(
            TaskProposalOutcome::TaskTerminal {
                task: expected.task,
                progress: record.task.progress,
            },
        ));
    }
    if record.task.reference != expected || record.task.purpose != proposal.premise.purpose {
        return Ok(SteeringPreparation::Refused(
            TaskProposalOutcome::StalePremise {
                current: record.task.reference,
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
        TaskCommitOutcome::Superseded => TaskProposalOutcome::Superseded,
        TaskCommitOutcome::TaskTerminal { task, progress } => {
            TaskProposalOutcome::TaskTerminal { task, progress }
        }
        TaskCommitOutcome::MissingTask { task } => TaskProposalOutcome::MissingTask { task },
        TaskCommitOutcome::RevisionExhausted { task } => {
            TaskProposalOutcome::RevisionExhausted { task }
        }
        TaskCommitOutcome::HeldForErasure => TaskProposalOutcome::HeldForErasure,
    }
}

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

pub async fn orchestrate_result_arrival(
    repository: &impl TaskRepository,
    delegation: DelegationId,
    body: TaskResultScrubPremise,
) -> Result<TaskResultArrivalOutcome, TaskTechnicalError> {
    repository
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation,
            result: TaskResultId::generate(),
            body,
        })
        .await
}
