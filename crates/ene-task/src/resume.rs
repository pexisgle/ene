use ene_primitive::{RawId, WallClockWithTz};

use crate::context::TaskContextEntryId;
use crate::delegation::{DelegationId, DelegationRef, TaskAgentEphemeralId};
use crate::repository::{OwnerMessageCurrentness, TaskRepository, TaskTechnicalError};
use crate::result::TaskResultId;
use crate::task::{SteeringPremiseRef, TaskId, TaskProgress, TaskRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTaskCommand {
    pub premise: SteeringPremiseRef,
    pub instruction: ResumeInstructionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResumeInstructionSource {
    OwnerHistory {
        message: RawId,
        currentness: OwnerMessageCurrentness,
    },
    OwnerManagement {
        activity: RawId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResumeOutcome {
    Resumed {
        task: TaskRef,
        delegation: DelegationRef,
    },
    StalePremise {
        current: TaskRef,
    },
    Superseded,
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    AlreadyRunning {
        task: TaskId,
    },
    HeldByUnknownEffects {
        task: TaskId,
    },
    ResultAvailable {
        task: TaskId,
    },
    NeedsRevalidation(TaskResumeHold),
    MissingTask {
        task: TaskId,
    },
    RevisionExhausted {
        task: TaskId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskResumeHold {
    CompanionUnavailable,
    WorkspaceUnavailable,
    InstructionUnavailable,
    PermissionUnavailable,
    DataUseHeld,
    ExecutionUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskResumeReadiness {
    pub permission_available: bool,
    pub execution_free: bool,
    pub launch_possible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResumeCommitPremise {
    pub command: ResumeTaskCommand,
    pub readiness: TaskResumeReadiness,
    pub adopted_purpose_entry: TaskContextEntryId,
    pub adopted_instruction_entry: TaskContextEntryId,
    pub delegation: DelegationId,
    pub agent: TaskAgentEphemeralId,
    pub accepted_at: WallClockWithTz,
}

#[must_use]
pub fn resume_commit_premise(
    command: ResumeTaskCommand,
    readiness: TaskResumeReadiness,
) -> TaskResumeCommitPremise {
    TaskResumeCommitPremise {
        command,
        readiness,
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction_entry: TaskContextEntryId::generate(),
        delegation: DelegationId::generate(),
        agent: TaskAgentEphemeralId::generate(),
        accepted_at: WallClockWithTz::now(),
    }
}

pub async fn orchestrate_resume(
    repository: &impl TaskRepository,
    command: ResumeTaskCommand,
    readiness: TaskResumeReadiness,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    let outcome = repository
        .commit_task_resume(resume_commit_premise(command, readiness))
        .await?;
    if let TaskResumeOutcome::ResultAvailable { task } = outcome {
        route_available_result(repository, task).await?;
    }
    Ok(outcome)
}

pub async fn orchestrate_resume_current(
    repository: &impl crate::ConversationTaskRepository,
    command: ResumeTaskCommand,
    readiness: TaskResumeReadiness,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    let outcome = repository
        .commit_task_resume_from_conversation(
            resume_commit_premise(command, readiness),
            currentness,
        )
        .await?;
    if let TaskResumeOutcome::ResultAvailable { task } = outcome {
        route_available_result(repository, task).await?;
    }
    Ok(outcome)
}

pub async fn route_available_result(
    repository: &impl TaskRepository,
    task: TaskId,
) -> Result<(), TaskTechnicalError> {
    use crate::report::TaskReportRowKind;

    let Some(record) = repository.load_task(task).await? else {
        return Ok(());
    };
    let current = record.task.reference;
    let mut after = None;
    loop {
        let page = repository
            .list_task_report_rows_after(task, after, crate::report::REPORT_PAGE_MAX)
            .await?;
        let full_page = page.len() as u32 == crate::report::REPORT_PAGE_MAX;
        after = page.last().map(|row| crate::report::TaskReportRowCursor {
            kind: row.kind,
            id: row.id,
        });
        for row in &page {
            if row.kind != TaskReportRowKind::TaskResult {
                continue;
            }
            let result = TaskResultId::from_raw(row.id);
            let Some(stored) = repository.load_task_result(result).await? else {
                continue;
            };
            if stored.adopted_revision.is_some() || stored.task != current {
                continue;
            }
            crate::reevaluate_result_adoption(repository, result).await?;
        }
        if !full_page {
            break;
        }
    }
    Ok(())
}
