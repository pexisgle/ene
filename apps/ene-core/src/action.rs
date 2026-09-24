use std::path::PathBuf;
use std::sync::Arc;

use ene_action::{
    ActionAttemptId, ActionClaimOutcome, ActionNotStarted, ActionRunOutcome, ActionTechnicalError,
    ObservedEffect, OperationKind, StartedWorkspaceAction, WorkspaceActionCommand,
    WorkspaceEffectRequest, WorkspaceEffectResponse, WorkspaceRoot, WorkspaceRootError,
    settle_workspace_effect, start_workspace_action,
};
use ene_inference::DispatchAbort;
use ene_permission::ActionEvaluationTracker;
use ene_primitive::RevisionInner;
use ene_store::Store;
use ene_task::{DelegationId, TaskId, TaskProgress, TaskRef, TaskRepository, TaskTechnicalError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::task_run::{TaskClaimKind, TaskExecutionRegistry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceActionHostOutcome {
    Completed {
        attempt: ActionAttemptId,
        effect: ObservedEffect,
        fact_recorded: bool,
    },
    NotStarted(ActionNotStarted),
    Stopped,
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
    #[error(transparent)]
    Task(#[from] TaskTechnicalError),
    #[error(transparent)]
    Action(#[from] ActionTechnicalError),
    #[error("workspace effect unavailable: {reason}")]
    EffectUnavailable { reason: String },
}

#[derive(Clone)]
pub(crate) struct TaskEffectRuntime {
    command: EffectWorkerCommand,
    active: Arc<tokio::sync::RwLock<()>>,
    hard_shutdown: DispatchAbort,
    live_workers: Arc<std::sync::atomic::AtomicUsize>,
    cooperative_abort: bool,
}

#[derive(Clone)]
struct EffectWorkerCommand {
    executable: PathBuf,
    args: Vec<String>,
    envs: Vec<(String, String)>,
}

enum TaskEffectExecution {
    Completed(ObservedEffect),
    Aborted,
}

impl TaskEffectRuntime {
    pub(crate) fn new() -> Self {
        Self::with_command(effect_worker_command(), true)
    }

    fn with_command(command: EffectWorkerCommand, cooperative_abort: bool) -> Self {
        Self {
            command,
            active: Arc::new(tokio::sync::RwLock::new(())),
            hard_shutdown: DispatchAbort::default(),
            live_workers: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            cooperative_abort,
        }
    }

    async fn execute(
        &self,
        started: &StartedWorkspaceAction,
        abort: &DispatchAbort,
    ) -> Result<TaskEffectExecution, WorkspaceActionHostError> {
        let _active = self.active.read().await;
        if self.hard_shutdown.is_aborted() || abort.is_aborted() {
            return Ok(TaskEffectExecution::Aborted);
        }
        let live = WorkerLiveGuard::new(&self.live_workers);
        let request = WorkspaceEffectRequest {
            root: started.root().as_path().to_string_lossy().into_owned(),
            target: started.target().as_path().to_owned(),
            operation: started.operation().as_str().to_owned(),
            content: started.content().map(<[u8]>::to_vec),
        };
        let encoded = serde_json::to_vec(&request).map_err(|error| {
            WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            }
        })?;
        let mut command = tokio::process::Command::new(&self.command.executable);
        command
            .args(&self.command.args)
            .env_clear()
            .envs(self.command.envs.iter().map(|(key, value)| (key, value)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child =
            command
                .spawn()
                .map_err(|error| WorkspaceActionHostError::EffectUnavailable {
                    reason: error.to_string(),
                })?;
        let Some(mut stdin) = child.stdin.take() else {
            terminate_child(&mut child).await;
            drop(live);
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: String::from("worker stdin unavailable"),
            });
        };
        if let Err(error) = stdin.write_all(&encoded).await {
            terminate_child(&mut child).await;
            drop(live);
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            });
        }
        drop(stdin);
        let Some(mut stdout) = child.stdout.take() else {
            terminate_child(&mut child).await;
            drop(live);
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: String::from("worker stdout unavailable"),
            });
        };
        let output = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let status = if self.cooperative_abort {
            tokio::select! {
                biased;
                () = abort.aborted() => {
                    terminate_child(&mut child).await;
                    drop(output.await);
                    drop(live);
                    return Ok(TaskEffectExecution::Aborted);
                }
                () = self.hard_shutdown.aborted() => {
                    terminate_child(&mut child).await;
                    drop(output.await);
                    drop(live);
                    return Ok(TaskEffectExecution::Aborted);
                }
                status = child.wait() => status,
            }
        } else {
            tokio::select! {
                biased;
                () = self.hard_shutdown.aborted() => {
                    terminate_child(&mut child).await;
                    drop(output.await);
                    drop(live);
                    return Ok(TaskEffectExecution::Aborted);
                }
                status = child.wait() => status,
            }
        };
        let bytes = output
            .await
            .map_err(|_| WorkspaceActionHostError::EffectUnavailable {
                reason: String::from("worker output task failed"),
            })?
            .map_err(|error| WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            })?;
        drop(live);
        if !status
            .map_err(|error| WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            })?
            .success()
        {
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: String::from("worker exited unsuccessfully"),
            });
        }
        let response: WorkspaceEffectResponse =
            serde_json::from_slice(&bytes).map_err(|error| {
                WorkspaceActionHostError::EffectUnavailable {
                    reason: error.to_string(),
                }
            })?;
        response
            .into_effect()
            .map(TaskEffectExecution::Completed)
            .map_err(|error| WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            })
    }

    pub(crate) async fn terminate_and_join(&self) {
        self.hard_shutdown.abort();
        let _active = self.active.write().await;
    }

    pub(crate) fn live_workers_for_tests(&self) -> usize {
        self.live_workers.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn for_test_command(
        executable: PathBuf,
        args: Vec<String>,
        envs: Vec<(String, String)>,
    ) -> Self {
        Self::with_command(
            EffectWorkerCommand {
                executable,
                args,
                envs,
            },
            false,
        )
    }
}

struct WorkerLiveGuard(Arc<std::sync::atomic::AtomicUsize>);

impl WorkerLiveGuard {
    fn new(live: &Arc<std::sync::atomic::AtomicUsize>) -> Self {
        live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(Arc::clone(live))
    }
}

impl Drop for WorkerLiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn terminate_child(child: &mut tokio::process::Child) {
    drop(child.kill().await);
    drop(child.wait().await);
}

fn effect_worker_command() -> EffectWorkerCommand {
    let executable = PathBuf::from(format!("ene-action-worker{}", std::env::consts::EXE_SUFFIX));
    let Ok(current) = std::env::current_exe() else {
        return EffectWorkerCommand {
            executable,
            args: Vec::new(),
            envs: Vec::new(),
        };
    };
    let beside = current.with_file_name(&executable);
    if beside.is_file() {
        return EffectWorkerCommand {
            executable: beside,
            args: Vec::new(),
            envs: Vec::new(),
        };
    }
    let parent = current
        .parent()
        .and_then(std::path::Path::parent)
        .map(|parent| parent.join(&executable));
    EffectWorkerCommand {
        executable: parent.unwrap_or(beside),
        args: Vec::new(),
        envs: Vec::new(),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the action boundary keeps the operation payload separate from the execution and shutdown authorities"
)]
pub(crate) async fn run_workspace_action(
    store: &Store,
    executions: &TaskExecutionRegistry,
    effect_runtime: &TaskEffectRuntime,
    abort: &DispatchAbort,
    delegation: DelegationId,
    operation: OperationKind,
    requested_path: String,
    content: Option<Vec<u8>>,
) -> Result<WorkspaceActionHostOutcome, WorkspaceActionHostError> {
    let Some(correspondence) = store.load_delegation(delegation).await? else {
        return Ok(WorkspaceActionHostOutcome::MissingDelegation { delegation });
    };
    let task = correspondence.task.task;
    let Some(record) = store.load_task(task).await? else {
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
    if store.load_delegation_result(delegation).await?.is_some() {
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
    let claim_scope = executions.task_claim_scope(TaskClaimKind::Action).await;
    if abort.is_aborted() {
        drop(claim_scope);
        return Ok(WorkspaceActionHostOutcome::Stopped);
    }
    let claim = start_workspace_action(store, &mut tracker, command).await?;
    drop(claim_scope);
    let started = match claim {
        ActionClaimOutcome::Started(started) => started,
        ActionClaimOutcome::NotStarted(ActionNotStarted::TaskTerminal) => {
            return match store.load_task(task).await? {
                Some(record) => Ok(WorkspaceActionHostOutcome::TaskTerminal {
                    task,
                    progress: record.task.progress,
                }),
                None => Ok(WorkspaceActionHostOutcome::MissingTask { task }),
            };
        }
        ActionClaimOutcome::NotStarted(ActionNotStarted::ExecutionSealed) => {
            return Ok(WorkspaceActionHostOutcome::ExecutionSealed { delegation });
        }
        ActionClaimOutcome::NotStarted(reason) => {
            return Ok(WorkspaceActionHostOutcome::NotStarted(reason));
        }
    };
    let attempt = started.attempt();
    let effect = match effect_runtime.execute(&started, abort).await? {
        TaskEffectExecution::Completed(effect) => effect,
        TaskEffectExecution::Aborted => return Ok(WorkspaceActionHostOutcome::Stopped),
    };
    executions.pause_after_task_effect_for_tests().await;
    match settle_workspace_effect(store, attempt, effect).await? {
        ActionRunOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        } => Ok(WorkspaceActionHostOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        }),
        ActionRunOutcome::NotStarted(reason) => Ok(WorkspaceActionHostOutcome::NotStarted(reason)),
    }
}
