//! Host adapter from the Task-owned delegation/workspace correspondence to
//! the Action-owned filesystem boundary.
//!
//! Composition only: this module loads the delegation correspondence and the
//! current Task unit, opens the current workspace association's folder, maps
//! the identities into the Action owner's opaque premise, and mirrors the
//! Action outcome without adding behavior. The authoritative premise compare
//! happens inside the Action repository's start transaction, and path
//! containment plus execution happen inside `ene-action`.
//!
//! A precheck here (missing row, moved revision, missing workspace) only
//! shapes the caller-facing domain answer; it is never the concurrency
//! guarantee.

use ene_action::{
    ActionAttemptId, ActionNotStarted, ActionRunOutcome, ActionTechnicalError, ObservedEffect,
    OperationKind, WorkspaceActionCommand, WorkspaceRoot, WorkspaceRootError,
    orchestrate_workspace_action,
};
use ene_permission::ActionEvaluationTracker;
use ene_primitive::RevisionInner;
use ene_store::Store;
use ene_task::{DelegationId, TaskId, TaskProgress, TaskRef, TaskRepository, TaskTechnicalError};

/// The caller-facing outcome of one Host filesystem action request.
///
/// Every variant is a domain answer: no provider-style technical error is
/// folded in, and no variant claims Task completion or result adoption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceActionHostOutcome {
    /// The attempt started and its effect was observed.
    Completed {
        attempt: ActionAttemptId,
        effect: ObservedEffect,
        /// `false` means the observation could not be stored; the durable row
        /// keeps its `Unknown` value and the effect is still reported.
        fact_recorded: bool,
    },
    /// The Action owner refused before or at the start claim.
    NotStarted(ActionNotStarted),
    /// The delegation correspondence does not exist.
    MissingDelegation { delegation: DelegationId },
    /// The delegated Task has no durable state.
    MissingTask { task: TaskId },
    /// The relied Task revision moved; nothing was claimed or executed.
    StaleTaskRevision { current: TaskRef },
    /// The Task is terminal (`Completed` / `Failed`); nothing was claimed or
    /// executed.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    /// The delegation already submitted its final result; nothing was claimed
    /// or executed, even while the Task is not terminal.
    ExecutionSealed { delegation: DelegationId },
    /// The Task has no current workspace association.
    MissingWorkspace { task: TaskId },
    /// The association exists but its folder is currently unusable.
    WorkspaceUnavailable { task: TaskId },
}

/// Technical failure of one Host filesystem action request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceActionHostError {
    #[error("task storage unavailable: {reason}")]
    TaskUnavailable { reason: String },
    #[error("action storage unavailable: {reason}")]
    ActionUnavailable { reason: String },
}

/// Runs one Workspace-contained filesystem action under an existing
/// delegation.
///
/// The current Task revision must still equal the delegation's relied
/// revision, and the current workspace association is the authority (the
/// delegation's copied scope is provenance, compared again inside the start
/// transaction). The folder is opened at request time; a vanished or
/// non-directory folder refuses without an attempt.
///
/// The association's optional `save_target` is not consulted here: it gates
/// the final save confirmation, which the Work owner decides in a later
/// slice. This request only constrains the path to the workspace folder.
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
    // The precheck reports the terminal lifecycle and the execution seal with
    // their own Task-side context; the authoritative refusal still happens
    // inside the AU5 claim, so a race after this read is refused there.
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
        // The Action owner's refusal vocabulary is unit-style: it holds no
        // Task lifecycle type. The Host explains the two Task-side refusals by
        // re-reading the durable Task (no delete path exists, so a missing row
        // here would mean the premise was never durable).
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
