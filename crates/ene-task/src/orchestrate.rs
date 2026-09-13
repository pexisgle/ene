//! Steering and delegation orchestration (H-A): premise precheck, identity
//! minting, and repository outcome handling.

use ene_primitive::{RawId, WallClockWithTz};

use crate::agent::TaskAgentOutput;
use crate::context::{TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind};
use crate::delegation::{
    CreateDelegationCommand, DelegationCreationPremise, DelegationId, DelegationOutcome,
    TaskAgentEphemeralId,
};
use crate::repository::{TaskCommitOutcome, TaskRepository, TaskTechnicalError};
use crate::result::{TaskAgentResultArrival, TaskResultId, TaskResultRecord};
use crate::task::{
    SteeringPremiseRef, TaskCommitPremise, TaskId, TaskInstructionAdoptionPremise, TaskProgress,
    TaskPurpose, TaskPurposeAdoptionPremise, TaskRef,
};

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

/// The Task owner's domain result for one steering proposal (H-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskProposalOutcome {
    /// The steering commit created exactly one new revision.
    AcceptedAsSteering(TaskRef),
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
    let expected = proposal.premise.expected;
    let Some(record) = repository.load_task(expected.task).await? else {
        return Ok(TaskProposalOutcome::MissingTask {
            task: expected.task,
        });
    };
    if record.task.reference != expected || record.task.purpose != proposal.premise.purpose {
        return Ok(TaskProposalOutcome::StalePremise {
            current: record.task.reference,
        });
    }
    if record.task.progress.is_terminal() {
        return Ok(TaskProposalOutcome::TaskTerminal {
            task: expected.task,
            progress: record.task.progress,
        });
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
    let outcome = repository
        .forward_steering(TaskCommitPremise {
            expected,
            new_purpose,
            adopted_purpose_entry,
            adopted_instruction,
        })
        .await?;
    Ok(match outcome {
        TaskCommitOutcome::CommittedAs(reference) => {
            TaskProposalOutcome::AcceptedAsSteering(reference)
        }
        TaskCommitOutcome::StaleExpected { current } => {
            TaskProposalOutcome::StalePremise { current }
        }
        TaskCommitOutcome::TaskTerminal { task, progress } => {
            TaskProposalOutcome::TaskTerminal { task, progress }
        }
        TaskCommitOutcome::MissingTask { task } => TaskProposalOutcome::MissingTask { task },
        TaskCommitOutcome::RevisionExhausted { task } => {
            TaskProposalOutcome::RevisionExhausted { task }
        }
    })
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
/// [`TaskAgentTurnOutcome::Produced`] is only a provider output and may still
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
