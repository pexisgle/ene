use ene_action::{
    ActionAttemptId, ActionNotStarted, ActionRunOutcome, ActionTechnicalError, ObservedEffect,
    OperationKind, WorkspaceActionCommand, WorkspaceRoot, WorkspaceRootError,
    orchestrate_workspace_action,
};
use ene_permission::ActionEvaluationTracker;
use ene_primitive::RevisionInner;
use ene_store::Store;
use ene_task::{DelegationId, TaskId, TaskProgress, TaskRef, TaskRepository, TaskTechnicalError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceActionHostOutcome {
    Completed {
        attempt: ActionAttemptId,
        effect: ObservedEffect,
        fact_recorded: bool,
    },
    NotStarted(ActionNotStarted),
    MissingDelegation {
        delegation: DelegationId,
    },
    MissingTask {
        task: TaskId,
    },
    StaleTaskRevision {
        current: TaskRef,
    },
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    ExecutionSealed {
        delegation: DelegationId,
    },
    MissingWorkspace {
        task: TaskId,
    },
    WorkspaceUnavailable {
        task: TaskId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceActionHostError {
    #[error("task storage unavailable: {reason}")]
    TaskUnavailable { reason: String },
    #[error("action storage unavailable: {reason}")]
    ActionUnavailable { reason: String },
}

pub async fn run_workspace_action(
    store: &Store,
    delegation: DelegationId,
    operation: OperationKind,
    requested_path: String,
    content: Option<Vec<u8>>,
) -> Result<WorkspaceActionHostOutcome, WorkspaceActionHostError> {
    let Some(correspondence) = store
        .load_delegation(delegation)
        .await
        .map_err(task_unavailable)?
    else {
        return Ok(WorkspaceActionHostOutcome::MissingDelegation { delegation });
    };
    let task = correspondence.task.task;
    let Some(record) = store.load_task(task).await.map_err(task_unavailable)? else {
        return Ok(WorkspaceActionHostOutcome::MissingTask { task });
    };
    if record.task.reference != correspondence.task {
        return Ok(WorkspaceActionHostOutcome::StaleTaskRevision {
            current: record.task.reference,
        });
    }
    if record.task.progress.is_terminal() {
        return Ok(WorkspaceActionHostOutcome::TaskTerminal {
            task,
            progress: record.task.progress,
        });
    }
    if store
        .load_delegation_result(delegation)
        .await
        .map_err(task_unavailable)?
        .is_some()
    {
        return Ok(WorkspaceActionHostOutcome::ExecutionSealed { delegation });
    }
    let Some(workspace) = record.workspace else {
        return Ok(WorkspaceActionHostOutcome::MissingWorkspace { task });
    };
    let root = match WorkspaceRoot::open(&workspace.folder.path) {
        Ok(root) => root,
        Err(WorkspaceRootError::Unavailable | WorkspaceRootError::NotADirectory) => {
            return Ok(WorkspaceActionHostOutcome::WorkspaceUnavailable { task });
        }
    };
    let command = WorkspaceActionCommand {
        delegation: delegation.as_raw(),
        task: task.as_raw(),
        task_revision: RevisionInner::from_u64(correspondence.task.revision.as_u64()),
        workspace: workspace.assoc.as_raw(),
        root,
        operation,
        requested_path,
        content,
    };
    let mut tracker = ActionEvaluationTracker::new();
    match orchestrate_workspace_action(store, &mut tracker, command)
        .await
        .map_err(action_unavailable)?
    {
        ActionRunOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        } => Ok(WorkspaceActionHostOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        }),
        ActionRunOutcome::NotStarted(ActionNotStarted::TaskTerminal) => {
            match store.load_task(task).await.map_err(task_unavailable)? {
                Some(record) => Ok(WorkspaceActionHostOutcome::TaskTerminal {
                    task,
                    progress: record.task.progress,
                }),
                None => Ok(WorkspaceActionHostOutcome::MissingTask { task }),
            }
        }
        ActionRunOutcome::NotStarted(ActionNotStarted::ExecutionSealed) => {
            Ok(WorkspaceActionHostOutcome::ExecutionSealed { delegation })
        }
        ActionRunOutcome::NotStarted(reason) => Ok(WorkspaceActionHostOutcome::NotStarted(reason)),
    }
}

fn task_unavailable(error: TaskTechnicalError) -> WorkspaceActionHostError {
    match error {
        TaskTechnicalError::StorageUnavailable { reason } => {
            WorkspaceActionHostError::TaskUnavailable { reason }
        }
    }
}

fn action_unavailable(error: ActionTechnicalError) -> WorkspaceActionHostError {
    match error {
        ActionTechnicalError::StorageUnavailable { reason } => {
            WorkspaceActionHostError::ActionUnavailable { reason }
        }
    }
}
