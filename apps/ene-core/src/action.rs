use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ene_action::{
    ActionAttemptId, ActionClaimOutcome, ActionNotStarted, ActionRunOutcome, ActionTechnicalError,
    ObservedEffect, OperationKind, StartedWorkspaceAction, WORKSPACE_EFFECT_PROTOCOL_GENERATION,
    WorkspaceActionCommand, WorkspaceEffectHandshake, WorkspaceEffectRequest,
    WorkspaceEffectResponse, WorkspaceRoot, WorkspaceRootError, settle_workspace_effect,
    start_workspace_action,
};
use ene_inference::DispatchAbort;
use ene_permission::ActionEvaluationTracker;
use ene_primitive::RevisionInner;
use ene_store::Store;
use ene_task::{DelegationId, TaskId, TaskProgress, TaskRef, TaskRepository, TaskTechnicalError};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use crate::staging_cleanup::{
    STAGING_HELPER_MODE_ENV, StagingHelperRequest, StagingHelperResponse,
};
use crate::task_run::{TaskClaimKind, TaskExecutionRegistry};

const WORKER_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_WORKER_HANDSHAKE_BYTES: usize = 1024;

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
    #[error("workspace worker unavailable: {reason}")]
    WorkerUnavailable { reason: String },
    #[error("workspace worker protocol mismatch: expected generation {expected}, got {actual:?}")]
    WorkerProtocolMismatch { expected: u32, actual: Option<u32> },
    #[error("workspace worker handshake failed: {reason}")]
    WorkerHandshakeFailed { reason: String },
}

#[derive(Clone)]
pub(crate) struct TaskEffectRuntime {
    command: EffectWorkerCommand,
    workers: Arc<StdMutex<WorkerRegistry>>,
    #[cfg(feature = "test-support")]
    test_staging_pause: Arc<StdMutex<Option<ene_action::WorkspaceEffectStagingPause>>>,
    #[cfg(feature = "test-support")]
    test_cleanup_pause: Arc<StdMutex<Option<crate::staging_cleanup::StagingCleanupPause>>>,
    #[cfg(feature = "test-support")]
    test_cleanup_failure: Arc<AtomicBool>,
    #[cfg(feature = "test-support")]
    test_staging_helper_pause: Arc<StdMutex<Option<StagingHelperPause>>>,
    #[cfg(feature = "test-support")]
    cooperative_abort: bool,
}

#[derive(Clone)]
struct EffectWorkerCommand {
    executable: PathBuf,
    args: Vec<String>,
    #[cfg(feature = "test-support")]
    envs: Vec<(String, String)>,
    #[cfg(feature = "test-support")]
    staging_executable: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    stdin_write_marker: Option<PathBuf>,
}

#[derive(Default)]
struct WorkerRegistry {
    closing: bool,
    next_id: u64,
    workers: HashMap<u64, Arc<WorkerHandle>>,
}

struct WorkerHandle {
    id: u64,
    pid: u32,
    process: Arc<ProcessHandle>,
    staging_obligation: StdMutex<Option<StagingCleanupObligation>>,
    staging_reaped: AtomicBool,
    finalizing: AtomicBool,
    reaped: AtomicBool,
    hard_stopped: AtomicBool,
    hard_stop_signal: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone)]
struct StagingPreparation {
    root: WorkspaceRoot,
    path: PathBuf,
    ownership_token: String,
}

struct StagingProcessIdentity {
    pid: u32,
    process: Arc<ProcessHandle>,
}

struct StagingHelperControl {
    child: tokio::sync::Mutex<tokio::process::Child>,
    identity: Arc<StagingProcessIdentity>,
    reaped: AtomicBool,
}

struct StagingHelperRuntime {
    control: Arc<StagingHelperControl>,
    stdout: BufReader<tokio::process::ChildStdout>,
}

struct PreparedStaging {
    preparation: StagingPreparation,
    runtime: StagingHelperRuntime,
}

struct StagingCleanupObligation {
    preparation: StagingPreparation,
    identity: Option<String>,
    control: Option<Arc<StagingHelperControl>>,
    prepared: bool,
}

#[cfg(feature = "test-support")]
#[derive(Clone)]
struct StagingHelperPause {
    stage: String,
    entered: PathBuf,
    release: PathBuf,
    canary: Option<PathBuf>,
}

#[derive(Debug)]
enum TaskEffectExecution {
    Completed(ObservedEffect),
    Aborted,
}

enum SpawnedWorker {
    Started {
        child: tokio::process::Child,
        handle: Arc<WorkerHandle>,
    },
    Closing,
}

struct PreparedWorker {
    child: tokio::process::Child,
    handle: Arc<WorkerHandle>,
    stdout: BufReader<tokio::process::ChildStdout>,
    staging: Option<PreparedStaging>,
}

impl TaskEffectRuntime {
    pub(crate) fn new() -> Self {
        Self::with_command(effect_worker_command())
    }

    fn with_command(command: EffectWorkerCommand) -> Self {
        Self {
            command,
            workers: Arc::new(StdMutex::new(WorkerRegistry::default())),
            #[cfg(feature = "test-support")]
            test_staging_pause: Arc::new(StdMutex::new(None)),
            #[cfg(feature = "test-support")]
            test_cleanup_pause: Arc::new(StdMutex::new(None)),
            #[cfg(feature = "test-support")]
            test_cleanup_failure: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "test-support")]
            test_staging_helper_pause: Arc::new(StdMutex::new(None)),
            #[cfg(feature = "test-support")]
            cooperative_abort: true,
        }
    }

    fn ensure_available(&self) -> Result<(), WorkspaceActionHostError> {
        if worker_executable_available(&self.command.executable) {
            return Ok(());
        }
        Err(WorkspaceActionHostError::WorkerUnavailable {
            reason: format!(
                "worker executable is unavailable: {}",
                self.command.executable.display()
            ),
        })
    }

    async fn prepare_worker(
        &self,
        abort: &DispatchAbort,
        preparation: Option<StagingPreparation>,
    ) -> Result<Option<PreparedWorker>, WorkspaceActionHostError> {
        self.ensure_available()?;
        let mut command = self.process_command(false);
        let spawned = self
            .spawn_registered(&mut command)
            .await
            .map_err(|reason| WorkspaceActionHostError::WorkerUnavailable { reason })?;
        let SpawnedWorker::Started { mut child, handle } = spawned else {
            return Ok(None);
        };
        let stdout = tokio::select! {
            biased;
            () = abort.aborted() => {
                self.stop_registered_child(&mut child, &handle).await;
                return self
                    .finish_worker(&handle)
                    .map(|()| None)
                    .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason });
            }
            result = negotiate_worker(&mut child) => match result {
                Ok(stdout) => stdout,
                Err(error) => {
                    self.stop_registered_child(&mut child, &handle).await;
                    if let Err(reason) = self.finish_worker(&handle) {
                        return Err(WorkspaceActionHostError::EffectUnavailable {
                            reason: format!("{}; {reason}", error),
                        });
                    }
                    return Err(error);
                }
            }
        };
        let Some(preparation) = preparation else {
            return Ok(Some(PreparedWorker {
                child,
                handle,
                stdout,
                staging: None,
            }));
        };
        if abort.is_aborted() || self.is_closing() {
            self.stop_registered_child(&mut child, &handle).await;
            return self
                .finish_worker(&handle)
                .map(|()| None)
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason });
        }
        self.register_staging_obligation(&handle, preparation.clone())
            .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
        let mut helper_command = self.process_command(true);
        let runtime = match self
            .spawn_staging_helper(&mut helper_command, &handle, &preparation)
            .await
        {
            Ok(runtime) => runtime,
            Err(reason) => {
                self.stop_registered_child(&mut child, &handle).await;
                let cleanup = self.settle_worker_obligation(&handle).await;
                return Err(WorkspaceActionHostError::EffectUnavailable {
                    reason: [Some(reason), cleanup.err()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("; "),
                });
            }
        };
        Ok(Some(PreparedWorker {
            child,
            handle,
            stdout,
            staging: Some(PreparedStaging {
                preparation,
                runtime,
            }),
        }))
    }

    async fn stop_registered_child(
        &self,
        child: &mut tokio::process::Child,
        handle: &Arc<WorkerHandle>,
    ) {
        stop_child(child).await;
        handle.reaped.store(true, Ordering::SeqCst);
    }

    async fn stop_prepared(&self, mut prepared: PreparedWorker) -> Result<(), String> {
        let handle = Arc::clone(&prepared.handle);
        self.stop_registered_child(&mut prepared.child, &handle)
            .await;
        let staging_result = self.finish_staging(handle.clone(), prepared.staging).await;
        let finish_result = self.finish_worker(&handle);
        staging_result.and(finish_result)
    }

    #[cfg(all(test, feature = "test-support"))]
    async fn execute(
        &self,
        started: &StartedWorkspaceAction,
        abort: &DispatchAbort,
    ) -> Result<TaskEffectExecution, WorkspaceActionHostError> {
        self.execute_with_preparation(
            started,
            abort,
            self.staging_preparation_for_started(started),
        )
        .await
    }

    #[cfg(all(test, feature = "test-support"))]
    async fn execute_with_preparation(
        &self,
        started: &StartedWorkspaceAction,
        abort: &DispatchAbort,
        preparation: Option<StagingPreparation>,
    ) -> Result<TaskEffectExecution, WorkspaceActionHostError> {
        let prepared = match self.prepare_worker(abort, preparation).await {
            Ok(Some(prepared)) => prepared,
            Ok(None) => return Ok(TaskEffectExecution::Aborted),
            Err(_error) if self.is_closing() => return Ok(TaskEffectExecution::Aborted),
            Err(error) => return Err(error),
        };
        match self.execute_prepared(started, abort, prepared).await {
            Err(_error) if self.is_closing() => Ok(TaskEffectExecution::Aborted),
            result => result,
        }
    }

    async fn execute_prepared(
        &self,
        started: &StartedWorkspaceAction,
        abort: &DispatchAbort,
        prepared: PreparedWorker,
    ) -> Result<TaskEffectExecution, WorkspaceActionHostError> {
        let PreparedWorker {
            mut child,
            handle,
            mut stdout,
            staging,
        } = prepared;
        if abort.is_aborted() || self.is_closing() {
            self.stop_registered_child(&mut child, &handle).await;
            let cleanup = self.finish_staging(handle.clone(), staging).await;
            let finish = self.finish_worker(&handle);
            cleanup
                .and(finish)
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return Ok(TaskEffectExecution::Aborted);
        }

        let staging_path = staging
            .as_ref()
            .map(|staging| staging.preparation.path.clone());
        let (staging_ownership_token, staging_identity) = match staging.as_ref() {
            Some(staging) => {
                let identity = crate::lock_unpoison(&handle.staging_obligation)
                    .as_ref()
                    .and_then(|obligation| obligation.identity.clone())
                    .ok_or_else(|| WorkspaceActionHostError::EffectUnavailable {
                        reason: String::from("prepared staging identity unavailable"),
                    })?;
                (
                    Some(staging.preparation.ownership_token.clone()),
                    Some(identity),
                )
            }
            None => (None, None),
        };
        let request = WorkspaceEffectRequest {
            root: started.root().as_path().to_string_lossy().into_owned(),
            target: started.target().as_path().to_owned(),
            operation: started.operation().as_str().to_owned(),
            content: started.content().map(<[u8]>::to_vec),
            staging_directory: staging_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            staging_ownership_token,
            staging_identity,
            #[cfg(feature = "test-support")]
            test_pause_after_staging: None,
        };
        let encoded = serde_json::to_vec(&request).map_err(|error| {
            WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            }
        })?;
        #[cfg(feature = "test-support")]
        let mut encoded = encoded;
        #[cfg(feature = "test-support")]
        if let Some(pause) = self.test_staging_pause() {
            let mut value =
                serde_json::from_slice::<serde_json::Value>(&encoded).map_err(|error| {
                    WorkspaceActionHostError::EffectUnavailable {
                        reason: error.to_string(),
                    }
                })?;
            let object = value.as_object_mut().ok_or_else(|| {
                WorkspaceActionHostError::EffectUnavailable {
                    reason: String::from("workspace effect request is not an object"),
                }
            })?;
            object.insert(
                String::from("test_pause_after_staging"),
                serde_json::to_value(pause).map_err(|error| {
                    WorkspaceActionHostError::EffectUnavailable {
                        reason: error.to_string(),
                    }
                })?,
            );
            encoded = serde_json::to_vec(&value).map_err(|error| {
                WorkspaceActionHostError::EffectUnavailable {
                    reason: error.to_string(),
                }
            })?;
        }
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                self.stop_registered_child(&mut child, &handle).await;
                let cleanup = self.finish_staging(handle.clone(), staging).await;
                let finish = self.finish_worker(&handle);
                return Err(WorkspaceActionHostError::EffectUnavailable {
                    reason: [
                        Some(String::from("worker stdin unavailable")),
                        cleanup.err(),
                        finish.err(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; "),
                });
            }
        };
        let mut output = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });

        #[cfg(feature = "test-support")]
        if let Some(marker) = self.command.stdin_write_marker.as_ref()
            && let Err(error) = std::fs::write(marker, b"write-started")
        {
            drop(stdin);
            self.stop_registered_child(&mut child, &handle).await;
            output.abort();
            drop(output.await);
            let cleanup = self.finish_staging(handle.clone(), staging).await;
            let finish = self.finish_worker(&handle);
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: [Some(error.to_string()), cleanup.err(), finish.err()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }

        let mut write_error = None;
        let mut stop_requested = tokio::select! {
            biased;
            () = self.wait_for_abort(abort) => true,
            result = stdin.write_all(&encoded) => {
                if let Err(error) = result {
                    write_error = Some(error.to_string());
                }
                false
            }
        };
        drop(stdin);
        if stop_requested || write_error.is_some() {
            self.stop_registered_child(&mut child, &handle).await;
        }

        let status = if stop_requested || write_error.is_some() {
            child.wait().await
        } else {
            tokio::select! {
                biased;
                () = self.wait_for_abort(abort) => {
                    stop_requested = true;
                    self.stop_registered_child(&mut child, &handle).await;
                    child.wait().await
                }
                status = child.wait() => status,
            }
        };
        if status.is_ok() {
            handle.reaped.store(true, Ordering::SeqCst);
        }
        let mut hard_stop_signal = handle.hard_stop_signal.subscribe();
        let output = if stop_requested || handle.hard_stopped.load(Ordering::SeqCst) {
            output.abort();
            output.await
        } else {
            tokio::select! {
                biased;
                _ = hard_stop_signal.changed() => {
                    output.abort();
                    output.await
                }
                result = &mut output => result,
            }
        };
        let cleanup = self.finish_staging(handle.clone(), staging).await;
        let finish = self.finish_worker(&handle);

        if stop_requested || handle.hard_stopped.load(Ordering::SeqCst) {
            drop(cleanup);
            drop(finish);
            return Ok(TaskEffectExecution::Aborted);
        }
        cleanup
            .and(finish)
            .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
        if let Some(reason) = write_error {
            return Err(WorkspaceActionHostError::EffectUnavailable { reason });
        }
        let status = status.map_err(|error| WorkspaceActionHostError::EffectUnavailable {
            reason: error.to_string(),
        })?;
        if !status.success() {
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: String::from("worker exited unsuccessfully"),
            });
        }
        let bytes = output
            .map_err(|_| WorkspaceActionHostError::EffectUnavailable {
                reason: String::from("worker output task failed"),
            })?
            .map_err(|error| WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
            })?;
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

    fn process_command(&self, staging_helper: bool) -> tokio::process::Command {
        #[cfg(feature = "test-support")]
        let executable = if staging_helper
            && let Some(staging_executable) = self.command.staging_executable.as_ref()
        {
            staging_executable.as_path()
        } else {
            self.command.executable.as_path()
        };
        #[cfg(not(feature = "test-support"))]
        let executable = self.command.executable.as_path();
        #[cfg(feature = "test-support")]
        let args: &[String] =
            if staging_helper && self.command.staging_executable.as_ref().is_some() {
                &[]
            } else {
                &self.command.args
            };
        #[cfg(not(feature = "test-support"))]
        let args: &[String] = &self.command.args;
        let mut command = tokio::process::Command::new(executable);
        command
            .args(args)
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if staging_helper {
            command.env(STAGING_HELPER_MODE_ENV, "1");
        }
        #[cfg(feature = "test-support")]
        command.envs(self.command.envs.iter().map(|(key, value)| (key, value)));
        #[cfg(feature = "test-support")]
        if staging_helper {
            command.envs(self.test_helper_envs());
        }
        command
    }

    #[cfg(all(test, feature = "test-support"))]
    fn staging_preparation_for_started(
        &self,
        started: &StartedWorkspaceAction,
    ) -> Option<StagingPreparation> {
        if !matches!(
            started.operation(),
            OperationKind::Create | OperationKind::Edit
        ) {
            return None;
        }
        let target = Path::new(started.target().as_path());
        let parent = target.parent().unwrap_or_else(|| started.root().as_path());
        Some(self.new_staging_preparation(started.root().clone(), parent.to_path_buf()))
    }

    fn new_staging_preparation(&self, root: WorkspaceRoot, parent: PathBuf) -> StagingPreparation {
        let identifier = Uuid::new_v4().as_hyphenated().to_string();
        StagingPreparation {
            root,
            path: parent.join(".ene-action-staging").join(&identifier),
            ownership_token: identifier,
        }
    }

    fn staging_preparation_for_action(
        &self,
        root: &WorkspaceRoot,
        requested_path: &str,
        operation: OperationKind,
    ) -> Option<StagingPreparation> {
        if !matches!(operation, OperationKind::Create | OperationKind::Edit) {
            return None;
        }
        let requested = Path::new(requested_path);
        if requested.is_absolute() {
            return None;
        }
        let mut normalized = PathBuf::new();
        for component in requested.components() {
            match component {
                std::path::Component::Normal(name) => normalized.push(name),
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if !normalized.pop() {
                        return None;
                    }
                }
                std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
            }
        }
        let parent = if normalized.as_os_str().is_empty() {
            root.as_path().to_path_buf()
        } else {
            root.as_path().join(
                normalized
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| std::path::Path::new("")),
            )
        };
        Some(self.new_staging_preparation(root.clone(), parent))
    }

    fn is_closing(&self) -> bool {
        crate::lock_unpoison(&self.workers).closing
    }

    async fn spawn_registered(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<SpawnedWorker, String> {
        let mut child = {
            let registry = crate::lock_unpoison(&self.workers);
            if registry.closing {
                return Ok(SpawnedWorker::Closing);
            }
            command.spawn().map_err(|error| error.to_string())?
        };
        let Some(pid) = child.id() else {
            stop_child(&mut child).await;
            return Err(String::from("worker process has no process id"));
        };
        let process = match ProcessHandle::open(pid) {
            Ok(process) => Arc::new(process),
            Err(error) => {
                stop_child(&mut child).await;
                return Err(error);
            }
        };
        let handle = {
            let mut registry = crate::lock_unpoison(&self.workers);
            if registry.closing {
                None
            } else {
                let id = registry.next_id;
                registry.next_id = registry.next_id.wrapping_add(1);
                let (hard_stop_signal, _) = tokio::sync::watch::channel(false);
                let handle = Arc::new(WorkerHandle {
                    id,
                    pid,
                    process: Arc::clone(&process),
                    staging_obligation: StdMutex::new(None),
                    staging_reaped: AtomicBool::new(true),
                    finalizing: AtomicBool::new(false),
                    reaped: AtomicBool::new(false),
                    hard_stopped: AtomicBool::new(false),
                    hard_stop_signal,
                });
                registry.workers.insert(id, Arc::clone(&handle));
                Some(handle)
            }
        };
        let Some(handle) = handle else {
            stop_child(&mut child).await;
            return Ok(SpawnedWorker::Closing);
        };
        Ok(SpawnedWorker::Started { child, handle })
    }

    fn register_staging_obligation(
        &self,
        handle: &Arc<WorkerHandle>,
        preparation: StagingPreparation,
    ) -> Result<(), String> {
        let mut obligation = crate::lock_unpoison(&handle.staging_obligation);
        if obligation.is_some() {
            return Err(format!("worker {} already owns staging cleanup", handle.id));
        }
        *obligation = Some(StagingCleanupObligation {
            preparation,
            identity: None,
            control: None,
            prepared: false,
        });
        Ok(())
    }

    async fn spawn_staging_helper(
        &self,
        command: &mut tokio::process::Command,
        handle: &Arc<WorkerHandle>,
        preparation: &StagingPreparation,
    ) -> Result<StagingHelperRuntime, String> {
        let mut child = {
            let registry = crate::lock_unpoison(&self.workers);
            if registry.closing {
                return Err(String::from("staging helper admission is closing"));
            }
            command.spawn().map_err(|error| error.to_string())?
        };
        let Some(pid) = child.id() else {
            stop_child(&mut child).await;
            return Err(String::from("staging helper process has no process id"));
        };
        let process = match ProcessHandle::open(pid) {
            Ok(process) => Arc::new(process),
            Err(error) => {
                stop_child(&mut child).await;
                return Err(error);
            }
        };
        let control = Arc::new(StagingHelperControl {
            child: tokio::sync::Mutex::new(child),
            identity: Arc::new(StagingProcessIdentity {
                pid,
                process: Arc::clone(&process),
            }),
            reaped: AtomicBool::new(false),
        });
        let obligation_present = {
            let registry = crate::lock_unpoison(&self.workers);
            if registry.closing {
                false
            } else {
                let mut obligation = crate::lock_unpoison(&handle.staging_obligation);
                if let Some(obligation) = obligation.as_mut() {
                    obligation.control = Some(Arc::clone(&control));
                    handle.staging_reaped.store(false, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            }
        };
        if !obligation_present {
            let reason = staging_stop_reason(
                &control,
                String::from("staging obligation disappeared during helper start"),
            )
            .await;
            handle.staging_reaped.store(true, Ordering::SeqCst);
            return Err(reason);
        }
        let mut runtime = match negotiate_staging_helper(&control).await {
            Ok(stdout) => StagingHelperRuntime { control, stdout },
            Err(reason) => {
                let reason = staging_stop_reason(&control, reason).await;
                handle.staging_reaped.store(true, Ordering::SeqCst);
                return Err(reason);
            }
        };
        let request = StagingHelperRequest::Prepare {
            root: preparation.root.as_path().to_string_lossy().into_owned(),
            staging_directory: preparation.path.to_string_lossy().into_owned(),
            ownership_token: preparation.ownership_token.clone(),
        };
        let response = match tokio::time::timeout(
            WORKER_HANDSHAKE_TIMEOUT,
            send_staging_helper_request(&mut runtime, request),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(reason)) => {
                let reason = staging_stop_reason(&runtime.control, reason).await;
                handle.staging_reaped.store(true, Ordering::SeqCst);
                return Err(reason);
            }
            Err(_) => {
                let reason = staging_stop_reason(
                    &runtime.control,
                    String::from("staging helper preparation timed out"),
                )
                .await;
                handle.staging_reaped.store(true, Ordering::SeqCst);
                return Err(reason);
            }
        };
        match response {
            StagingHelperResponse::Prepared { identity } => {
                let mut obligation = crate::lock_unpoison(&handle.staging_obligation);
                if let Some(obligation) = obligation.as_mut() {
                    obligation.identity = Some(identity);
                    obligation.prepared = true;
                }
                Ok(runtime)
            }
            StagingHelperResponse::CleanupComplete { .. } => {
                let reason = String::from("staging helper returned cleanup before prepare");
                let reason = staging_stop_reason(&runtime.control, reason).await;
                handle.staging_reaped.store(true, Ordering::SeqCst);
                Err(reason)
            }
            StagingHelperResponse::Failed {
                reason,
                obligation_retained,
            } => {
                let stopped = hard_stop_staging_control(&runtime.control).await;
                handle.staging_reaped.store(true, Ordering::SeqCst);
                if !obligation_retained {
                    *crate::lock_unpoison(&handle.staging_obligation) = None;
                }
                match stopped {
                    Ok(()) => Err(reason),
                    Err(stop_reason) => Err(format!("{reason}; {stop_reason}")),
                }
            }
        }
    }

    async fn cleanup_staging_runtime(
        &self,
        runtime: &mut StagingHelperRuntime,
    ) -> Result<(), String> {
        if runtime.control.reaped.load(Ordering::SeqCst) {
            return Err(String::from(
                "staging helper was hard-stopped before cleanup",
            ));
        }
        let response = tokio::time::timeout(
            WORKER_HANDSHAKE_TIMEOUT,
            send_staging_helper_request(runtime, StagingHelperRequest::Cleanup),
        )
        .await
        .map_err(|_| String::from("staging helper cleanup timed out"))??;
        match response {
            StagingHelperResponse::CleanupComplete { removed: true } => Ok(()),
            StagingHelperResponse::CleanupComplete { removed: false } => {
                Err(String::from("staging helper cleanup found no owned object"))
            }
            StagingHelperResponse::Prepared { .. } => Err(String::from(
                "staging helper returned prepare during cleanup",
            )),
            StagingHelperResponse::Failed { reason, .. } => Err(reason),
        }
    }

    async fn finish_staging(
        &self,
        handle: Arc<WorkerHandle>,
        staging: Option<PreparedStaging>,
    ) -> Result<(), String> {
        let Some(mut staging) = staging else {
            return Ok(());
        };
        let cleanup = self.cleanup_staging_runtime(&mut staging.runtime).await;
        let stopped = hard_stop_staging_control(&staging.runtime.control).await;
        handle.staging_reaped.store(true, Ordering::SeqCst);
        let result = cleanup.and(stopped);
        if result.is_ok() {
            let mut obligation = crate::lock_unpoison(&handle.staging_obligation);
            *obligation = None;
        }
        result
    }

    fn finish_worker(&self, handle: &Arc<WorkerHandle>) -> Result<(), String> {
        if handle.finalizing.load(Ordering::SeqCst) {
            return Err(format!("worker {} finalization is in progress", handle.id));
        }
        if crate::lock_unpoison(&handle.staging_obligation).is_some() {
            return Err(format!(
                "worker {} cannot finish before staging cleanup",
                handle.id
            ));
        }
        if !handle.reaped.load(Ordering::SeqCst) || !handle.staging_reaped.load(Ordering::SeqCst) {
            return Err(format!("worker {} has a live process", handle.id));
        }
        let mut registry = crate::lock_unpoison(&self.workers);
        registry.workers.remove(&handle.id);
        Ok(())
    }

    pub(crate) fn close_admission(&self) {
        crate::lock_unpoison(&self.workers).closing = true;
    }

    pub(crate) async fn terminate_and_join(&self) -> Result<(), String> {
        let workers = {
            let mut registry = crate::lock_unpoison(&self.workers);
            registry.closing = true;
            registry.workers.values().cloned().collect::<Vec<_>>()
        };
        let mut failure = None;
        for worker in &workers {
            if let Err(error) = hard_kill_worker(worker) {
                failure.get_or_insert(error);
            }
            let control = crate::lock_unpoison(&worker.staging_obligation)
                .as_ref()
                .and_then(|obligation| obligation.control.as_ref())
                .cloned();
            if let Some(control) = control {
                if let Err(error) = hard_stop_staging_control(&control).await {
                    failure.get_or_insert(error);
                }
                worker.staging_reaped.store(true, Ordering::SeqCst);
            }
        }
        let settle_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while !crate::lock_unpoison(&self.workers).workers.is_empty()
            && tokio::time::Instant::now() < settle_deadline
        {
            tokio::task::yield_now().await;
        }
        if let Err(error) = self.finalize_shutdown_inner().await {
            failure.get_or_insert(error);
        }
        failure.map_or(Ok(()), Err)
    }

    pub(crate) fn verify_shutdown(&self) -> Result<(), String> {
        let workers = crate::lock_unpoison(&self.workers);
        let remaining = workers.workers.len();
        if remaining == 0 {
            Ok(())
        } else {
            Err(format!(
                "{remaining} worker process or cleanup obligation(s) remained"
            ))
        }
    }

    async fn settle_worker_obligation(&self, worker: &Arc<WorkerHandle>) -> Result<(), String> {
        if worker
            .finalizing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let obligation = crate::lock_unpoison(&worker.staging_obligation).take();
        let mut failure = None;
        if let Some(obligation) = obligation {
            match self.cleanup_obligation_by_path(&obligation).await {
                Ok(true) => {}
                Ok(false) if !obligation.prepared => {}
                Ok(false) => {
                    failure = Some(String::from(
                        "prepared staging object disappeared before cleanup",
                    ));
                    *crate::lock_unpoison(&worker.staging_obligation) = Some(obligation);
                }
                Err(reason) => {
                    failure = Some(reason);
                    *crate::lock_unpoison(&worker.staging_obligation) = Some(obligation);
                }
            }
        }
        worker.finalizing.store(false, Ordering::SeqCst);
        if let Some(reason) = failure {
            return Err(reason);
        }
        if let Err(reason) = self.finish_worker(worker)
            && reason != format!("worker {} has a live process", worker.id)
        {
            return Err(reason);
        }
        Ok(())
    }

    async fn finalize_shutdown_inner(&self) -> Result<(), String> {
        let workers = {
            let registry = crate::lock_unpoison(&self.workers);
            registry.workers.values().cloned().collect::<Vec<_>>()
        };
        let mut failure = None;
        for worker in workers {
            if worker
                .finalizing
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                continue;
            }
            let obligation = crate::lock_unpoison(&worker.staging_obligation).take();
            let mut unresolved = None;
            if let Some(obligation) = obligation {
                match self.cleanup_obligation_by_path(&obligation).await {
                    Ok(true) => {}
                    Ok(false) if !obligation.prepared => {}
                    Ok(false) => {
                        unresolved = Some(String::from(
                            "prepared staging object disappeared before cleanup",
                        ));
                        *crate::lock_unpoison(&worker.staging_obligation) = Some(obligation);
                    }
                    Err(reason) => {
                        unresolved = Some(reason);
                        *crate::lock_unpoison(&worker.staging_obligation) = Some(obligation);
                    }
                }
            }
            if let Some(reason) = unresolved {
                worker.finalizing.store(false, Ordering::SeqCst);
                failure.get_or_insert(reason);
                continue;
            }
            worker.finalizing.store(false, Ordering::SeqCst);
            if let Err(reason) = self.finish_worker(&worker)
                && reason != format!("worker {} has a live process", worker.id)
            {
                failure.get_or_insert(reason);
            }
        }
        let remaining = crate::lock_unpoison(&self.workers)
            .workers
            .values()
            .filter(|worker| crate::lock_unpoison(&worker.staging_obligation).is_some())
            .count();
        if remaining != 0 {
            failure.get_or_insert(format!("{remaining} worker cleanup obligation(s) remained"));
        }
        failure.map_or(Ok(()), Err)
    }

    async fn cleanup_obligation_by_path(
        &self,
        obligation: &StagingCleanupObligation,
    ) -> Result<bool, String> {
        self.cleanup_obligation_by_path_with_timeout(obligation, WORKER_HANDSHAKE_TIMEOUT)
            .await
    }

    async fn cleanup_obligation_by_path_with_timeout(
        &self,
        obligation: &StagingCleanupObligation,
        timeout: std::time::Duration,
    ) -> Result<bool, String> {
        let mut command = self.process_command(true);
        let mut runtime = self.spawn_staging_helper_unregistered(&mut command).await?;
        let request = StagingHelperRequest::CleanupByPath {
            root: obligation
                .preparation
                .root
                .as_path()
                .to_string_lossy()
                .into_owned(),
            staging_directory: obligation.preparation.path.to_string_lossy().into_owned(),
            ownership_token: obligation.preparation.ownership_token.clone(),
            identity: obligation.identity.clone(),
        };
        let result =
            tokio::time::timeout(timeout, send_staging_helper_request(&mut runtime, request)).await;
        // Always kill and reap the unregistered retry helper before returning,
        // including timeout and protocol/I/O error paths.
        let stopped = hard_stop_staging_control(&runtime.control).await;
        let response = match result {
            Ok(Ok(response)) => response,
            Ok(Err(reason)) => {
                return match stopped {
                    Ok(()) => Err(reason),
                    Err(stop_reason) => Err(format!("{reason}; {stop_reason}")),
                };
            }
            Err(_) => {
                let reason = String::from("staging obligation cleanup timed out");
                return match stopped {
                    Ok(()) => Err(reason),
                    Err(stop_reason) => Err(format!("{reason}; {stop_reason}")),
                };
            }
        };
        stopped?;
        match response {
            StagingHelperResponse::CleanupComplete { removed } => Ok(removed),
            StagingHelperResponse::Prepared { .. } => Err(String::from(
                "staging helper returned prepare during path cleanup",
            )),
            StagingHelperResponse::Failed { reason, .. } => Err(reason),
        }
    }

    async fn spawn_staging_helper_unregistered(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<StagingHelperRuntime, String> {
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let Some(pid) = child.id() else {
            stop_child(&mut child).await;
            return Err(String::from("staging helper process has no process id"));
        };
        let process = match ProcessHandle::open(pid) {
            Ok(process) => Arc::new(process),
            Err(error) => {
                stop_child(&mut child).await;
                return Err(error);
            }
        };
        let control = Arc::new(StagingHelperControl {
            child: tokio::sync::Mutex::new(child),
            identity: Arc::new(StagingProcessIdentity {
                pid,
                process: Arc::clone(&process),
            }),
            reaped: AtomicBool::new(false),
        });
        match negotiate_staging_helper(&control).await {
            Ok(stdout) => Ok(StagingHelperRuntime { control, stdout }),
            Err(reason) => Err(staging_stop_reason(&control, reason).await),
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn live_workers_for_tests(&self) -> usize {
        crate::lock_unpoison(&self.workers).workers.len()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn live_worker_processes_for_tests(&self) -> usize {
        crate::lock_unpoison(&self.workers)
            .workers
            .values()
            .map(|worker| {
                usize::from(!worker.reaped.load(Ordering::SeqCst))
                    + usize::from(
                        crate::lock_unpoison(&worker.staging_obligation)
                            .as_ref()
                            .and_then(|obligation| obligation.control.as_ref())
                            .is_some_and(|control| !control.reaped.load(Ordering::SeqCst)),
                    )
            })
            .sum()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn pending_staging_obligations_for_tests(&self) -> usize {
        crate::lock_unpoison(&self.workers)
            .workers
            .values()
            .filter(|worker| crate::lock_unpoison(&worker.staging_obligation).is_some())
            .count()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn for_test_command(
        executable: PathBuf,
        args: Vec<String>,
        envs: Vec<(String, String)>,
    ) -> Self {
        let staging_executable = std::env::current_exe()
            .ok()
            .and_then(|current| {
                current.parent().and_then(Path::parent).map(|parent| {
                    parent.join(format!("ene-action-worker{}", std::env::consts::EXE_SUFFIX))
                })
            })
            .filter(|path| path.is_file());
        let mut runtime = Self::with_command(EffectWorkerCommand {
            executable,
            args,
            envs,
            #[cfg(feature = "test-support")]
            staging_executable,
            #[cfg(feature = "test-support")]
            stdin_write_marker: None,
        });
        runtime.cooperative_abort = false;
        runtime
    }

    async fn wait_for_abort(&self, abort: &DispatchAbort) {
        #[cfg(feature = "test-support")]
        if !self.cooperative_abort {
            std::future::pending::<()>().await;
        }
        abort.aborted().await;
    }

    #[cfg(feature = "test-support")]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "used by the deterministic supervisor regression tests"
        )
    )]
    pub(crate) fn set_test_stdin_write_marker(&mut self, marker: PathBuf) {
        self.command.stdin_write_marker = Some(marker);
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn set_test_staging_pause(
        &self,
        pause: Option<ene_action::WorkspaceEffectStagingPause>,
    ) {
        *crate::lock_unpoison(&self.test_staging_pause) = pause;
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn set_test_cleanup_pause(
        &self,
        pause: Option<crate::staging_cleanup::StagingCleanupPause>,
    ) {
        *crate::lock_unpoison(&self.test_cleanup_pause) = pause;
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn set_test_cleanup_failure(&self, fail: bool) {
        self.test_cleanup_failure.store(fail, Ordering::SeqCst);
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn set_test_staging_helper_pause(
        &self,
        stage: &str,
        entered: PathBuf,
        release: PathBuf,
        canary: Option<PathBuf>,
    ) {
        *crate::lock_unpoison(&self.test_staging_helper_pause) = Some(StagingHelperPause {
            stage: stage.to_owned(),
            entered,
            release,
            canary,
        });
    }

    #[cfg(feature = "test-support")]
    fn test_staging_pause(&self) -> Option<ene_action::WorkspaceEffectStagingPause> {
        crate::lock_unpoison(&self.test_staging_pause).clone()
    }

    #[cfg(all(test, feature = "test-support"))]
    fn staging_lease_options(&self) -> crate::staging_cleanup::StagingLeaseOptions {
        crate::staging_cleanup::StagingLeaseOptions {
            #[cfg(feature = "test-support")]
            pause: crate::lock_unpoison(&self.test_cleanup_pause).clone(),
            #[cfg(feature = "test-support")]
            fail_cleanup: self.test_cleanup_failure.load(Ordering::SeqCst),
            ..crate::staging_cleanup::StagingLeaseOptions::default()
        }
    }

    #[cfg(feature = "test-support")]
    fn test_helper_envs(&self) -> Vec<(String, String)> {
        let mut envs = Vec::new();
        if let Some(pause) = crate::lock_unpoison(&self.test_cleanup_pause).as_ref() {
            envs.push((
                String::from("ENE_TEST_STAGING_CLEANUP_ENTERED"),
                pause.entered.to_string_lossy().into_owned(),
            ));
            envs.push((
                String::from("ENE_TEST_STAGING_CLEANUP_RELEASE"),
                pause.release.to_string_lossy().into_owned(),
            ));
        }
        if self.test_cleanup_failure.load(Ordering::SeqCst) {
            envs.push((
                String::from("ENE_TEST_STAGING_CLEANUP_FAILURE"),
                String::from("1"),
            ));
        }
        if let Some(pause) = crate::lock_unpoison(&self.test_staging_helper_pause).as_ref() {
            envs.push((
                String::from("ENE_TEST_STAGING_HELPER_PAUSE"),
                format!("stall-{}", pause.stage),
            ));
            envs.push((
                String::from("ENE_TEST_STAGING_HELPER_ENTERED"),
                pause.entered.to_string_lossy().into_owned(),
            ));
            envs.push((
                String::from("ENE_TEST_STAGING_HELPER_RELEASE"),
                pause.release.to_string_lossy().into_owned(),
            ));
            if pause.stage == "cleanup" {
                envs.push((
                    String::from("ENE_TEST_STAGING_CLEANUP_ENTERED"),
                    pause.entered.to_string_lossy().into_owned(),
                ));
                envs.push((
                    String::from("ENE_TEST_STAGING_CLEANUP_RELEASE"),
                    pause.release.to_string_lossy().into_owned(),
                ));
            }
            if let Some(canary) = pause.canary.as_ref() {
                envs.push((
                    String::from("ENE_TEST_STAGING_HELPER_CANARY"),
                    canary.to_string_lossy().into_owned(),
                ));
            }
        }
        envs
    }
}

async fn read_bounded_handshake_line(
    stdout: &mut BufReader<tokio::process::ChildStdout>,
) -> Result<String, WorkspaceActionHostError> {
    let mut line = Vec::with_capacity(MAX_WORKER_HANDSHAKE_BYTES);
    let read = stdout
        .take(MAX_WORKER_HANDSHAKE_BYTES as u64)
        .read_until(b'\n', &mut line)
        .await
        .map_err(|error| WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: error.to_string(),
        })?;
    if read == 0 {
        return Err(WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: String::from("worker closed stdout during handshake"),
        });
    }
    if line.len() > MAX_WORKER_HANDSHAKE_BYTES || line.last() != Some(&b'\n') {
        return Err(WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: String::from("worker handshake exceeds its size limit"),
        });
    }
    String::from_utf8(line).map_err(|error| WorkspaceActionHostError::WorkerHandshakeFailed {
        reason: error.to_string(),
    })
}

#[cfg(not(feature = "test-support"))]
async fn read_handshake_response(
    stdout: &mut BufReader<tokio::process::ChildStdout>,
) -> Result<WorkspaceEffectHandshake, WorkspaceActionHostError> {
    let line = read_bounded_handshake_line(stdout).await?;
    serde_json::from_str::<WorkspaceEffectHandshake>(line.trim()).map_err(|error| {
        WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: error.to_string(),
        }
    })
}

#[cfg(feature = "test-support")]
async fn read_handshake_response(
    stdout: &mut BufReader<tokio::process::ChildStdout>,
) -> Result<WorkspaceEffectHandshake, WorkspaceActionHostError> {
    loop {
        let line = read_bounded_handshake_line(stdout).await?;
        if let Some(start) = line.find('{') {
            return serde_json::from_str::<WorkspaceEffectHandshake>(&line[start..]).map_err(
                |error| WorkspaceActionHostError::WorkerHandshakeFailed {
                    reason: error.to_string(),
                },
            );
        }
    }
}

async fn negotiate_worker(
    child: &mut tokio::process::Child,
) -> Result<BufReader<tokio::process::ChildStdout>, WorkspaceActionHostError> {
    let Some(stdin) = child.stdin.as_mut() else {
        return Err(WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: String::from("worker stdin unavailable during handshake"),
        });
    };
    let handshake = serde_json::to_vec(&WorkspaceEffectHandshake {
        generation: WORKSPACE_EFFECT_PROTOCOL_GENERATION,
    })
    .map_err(|error| WorkspaceActionHostError::WorkerHandshakeFailed {
        reason: error.to_string(),
    })?;
    stdin.write_all(&handshake).await.map_err(|error| {
        WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: error.to_string(),
        }
    })?;
    stdin.write_all(b"\n").await.map_err(|error| {
        WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: error.to_string(),
        }
    })?;
    stdin
        .flush()
        .await
        .map_err(|error| WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: error.to_string(),
        })?;
    let Some(stdout) = child.stdout.take() else {
        return Err(WorkspaceActionHostError::WorkerHandshakeFailed {
            reason: String::from("worker stdout unavailable during handshake"),
        });
    };
    let mut stdout = BufReader::new(stdout);
    let response = tokio::time::timeout(
        WORKER_HANDSHAKE_TIMEOUT,
        read_handshake_response(&mut stdout),
    )
    .await
    .map_err(|_| WorkspaceActionHostError::WorkerHandshakeFailed {
        reason: String::from("worker did not answer the protocol handshake"),
    })??;
    if response.generation != WORKSPACE_EFFECT_PROTOCOL_GENERATION {
        return Err(WorkspaceActionHostError::WorkerProtocolMismatch {
            expected: WORKSPACE_EFFECT_PROTOCOL_GENERATION,
            actual: Some(response.generation),
        });
    }
    Ok(stdout)
}

async fn negotiate_staging_helper(
    control: &StagingHelperControl,
) -> Result<BufReader<tokio::process::ChildStdout>, String> {
    let handshake = serde_json::to_vec(&WorkspaceEffectHandshake {
        generation: WORKSPACE_EFFECT_PROTOCOL_GENERATION,
    })
    .map_err(|error| error.to_string())?;
    let stdout = {
        let mut child = control.child.lock().await;
        let Some(stdin) = child.stdin.as_mut() else {
            return Err(String::from(
                "staging helper stdin unavailable during handshake",
            ));
        };
        stdin
            .write_all(&handshake)
            .await
            .map_err(|error| error.to_string())?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|error| error.to_string())?;
        stdin.flush().await.map_err(|error| error.to_string())?;
        child
            .stdout
            .take()
            .ok_or_else(|| String::from("staging helper stdout unavailable during handshake"))?
    };
    let mut stdout = BufReader::new(stdout);
    let response = tokio::time::timeout(
        WORKER_HANDSHAKE_TIMEOUT,
        read_handshake_response(&mut stdout),
    )
    .await
    .map_err(|_| {
        format!(
            "staging helper {} did not answer the protocol handshake",
            control.identity.pid
        )
    })?
    .map_err(|error| format!("staging helper {} handshake: {error}", control.identity.pid))?;
    if response.generation != WORKSPACE_EFFECT_PROTOCOL_GENERATION {
        return Err(format!(
            "staging helper protocol generation mismatch: {}",
            response.generation
        ));
    }
    Ok(stdout)
}

async fn send_staging_helper_request(
    runtime: &mut StagingHelperRuntime,
    request: StagingHelperRequest,
) -> Result<StagingHelperResponse, String> {
    let encoded = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    {
        let mut child = runtime.control.child.lock().await;
        let Some(stdin) = child.stdin.as_mut() else {
            return Err(String::from("staging helper stdin unavailable"));
        };
        stdin
            .write_all(encoded.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|error| error.to_string())?;
        stdin.flush().await.map_err(|error| error.to_string())?;
    }
    let line = read_bounded_handshake_line(&mut runtime.stdout)
        .await
        .map_err(|error| format!("staging helper response unavailable: {error}"))?;
    serde_json::from_str(line.trim())
        .map_err(|error| format!("staging helper response malformed: {error}"))
}

async fn staging_stop_reason(control: &StagingHelperControl, reason: String) -> String {
    match hard_stop_staging_control(control).await {
        Ok(()) => reason,
        Err(stop_reason) => format!("{reason}; {stop_reason}"),
    }
}

async fn hard_stop_staging_control(control: &StagingHelperControl) -> Result<(), String> {
    if control.reaped.load(Ordering::SeqCst) {
        return Ok(());
    }
    let kill_result = control.identity.process.hard_kill(control.identity.pid);
    let mut child = control.child.lock().await;
    stop_child(&mut child).await;
    control.reaped.store(true, Ordering::SeqCst);
    kill_result
}

async fn stop_child(child: &mut tokio::process::Child) {
    drop(child.kill().await);
    drop(child.wait().await);
}

fn worker_executable_available(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn effect_worker_command() -> EffectWorkerCommand {
    let file_name = format!("ene-action-worker{}", std::env::consts::EXE_SUFFIX);
    let Ok(current) = std::env::current_exe() else {
        return EffectWorkerCommand {
            executable: PathBuf::from(&file_name),
            args: Vec::new(),
            #[cfg(feature = "test-support")]
            envs: Vec::new(),
            #[cfg(feature = "test-support")]
            staging_executable: None,
            #[cfg(feature = "test-support")]
            stdin_write_marker: None,
        };
    };
    effect_worker_command_at(
        &current,
        &file_name,
        cfg!(debug_assertions) || cfg!(feature = "test-support"),
    )
}

fn effect_worker_command_at(
    current: &Path,
    file_name: &str,
    allow_development_fallback: bool,
) -> EffectWorkerCommand {
    let beside = current.with_file_name(file_name);
    if beside.is_file() {
        return EffectWorkerCommand {
            executable: beside,
            args: Vec::new(),
            #[cfg(feature = "test-support")]
            envs: Vec::new(),
            #[cfg(feature = "test-support")]
            staging_executable: None,
            #[cfg(feature = "test-support")]
            stdin_write_marker: None,
        };
    }
    if allow_development_fallback && let Some(parent) = current.parent().and_then(Path::parent) {
        let beside = parent.join(file_name);
        if beside.is_file() {
            return EffectWorkerCommand {
                executable: beside,
                args: Vec::new(),
                #[cfg(feature = "test-support")]
                envs: Vec::new(),
                #[cfg(feature = "test-support")]
                staging_executable: None,
                #[cfg(feature = "test-support")]
                stdin_write_marker: None,
            };
        }
    }
    let host_executable = current
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name == "ene-core");
    if !allow_development_fallback || !host_executable {
        return EffectWorkerCommand {
            executable: beside,
            args: Vec::new(),
            #[cfg(feature = "test-support")]
            envs: Vec::new(),
            #[cfg(feature = "test-support")]
            staging_executable: None,
            #[cfg(feature = "test-support")]
            stdin_write_marker: None,
        };
    }
    EffectWorkerCommand {
        executable: current.to_path_buf(),
        args: vec![String::from("workspace-effect-worker")],
        #[cfg(feature = "test-support")]
        envs: Vec::new(),
        #[cfg(feature = "test-support")]
        staging_executable: None,
        #[cfg(feature = "test-support")]
        stdin_write_marker: None,
    }
}

// The registry keeps an OS process identity rather than a bare PID so a
// reaped PID can never be reused as a hard-stop target.
#[cfg(target_os = "linux")]
struct ProcessHandle {
    pidfd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl ProcessHandle {
    fn open(pid: u32) -> Result<Self, String> {
        use std::os::fd::FromRawFd as _;

        // SAFETY: pidfd_open only reads the supplied PID and returns a new
        // descriptor; it has no memory-safety preconditions.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(format!(
                "worker {pid} could not be opened as a pidfd: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: pidfd_open returned a new owned file descriptor, and the
        // cast is valid because Linux file descriptors are non-negative i32s.
        Ok(Self {
            // SAFETY: the successful pidfd_open call transferred ownership
            // of this descriptor to the OwnedFd value.
            pidfd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) },
        })
    }

    fn hard_kill(&self, pid: u32) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;

        // SAFETY: the pidfd remains owned by this handle for the syscall;
        // SIGKILL has no pointer preconditions and a null siginfo is valid.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(format!(
                "worker {pid} could not be killed through its pidfd: {error}"
            ))
        }
    }
}

#[cfg(windows)]
struct ProcessHandle {
    handle: Option<isize>,
}

#[cfg(windows)]
impl ProcessHandle {
    fn open(pid: u32) -> Result<Self, String> {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE};

        // SAFETY: the PID belongs to the child just spawned by this runtime;
        // OpenProcess returns an owned process handle on success.
        let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(87) {
                Ok(Self { handle: None })
            } else {
                Err(format!(
                    "worker {pid} could not be opened for hard stop: {error}"
                ))
            };
        }
        Ok(Self {
            handle: Some(process as isize),
        })
    }

    fn hard_kill(&self, pid: u32) -> Result<(), String> {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Threading::TerminateProcess;

        let Some(handle) = self.handle else {
            return Ok(());
        };
        // SAFETY: `handle` was obtained from OpenProcess and remains owned by
        // this ProcessHandle until after the termination call.
        let terminated = unsafe { TerminateProcess(handle as HANDLE, 1) };
        if terminated == 0 {
            let error = std::io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(5) | Some(87)) {
                return Err(format!(
                    "worker {pid} could not be terminated through its process handle: {error}"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

        let Some(handle) = self.handle else {
            return;
        };
        // SAFETY: the handle is owned by this value and is closed exactly once.
        unsafe { CloseHandle(handle as HANDLE) };
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
struct ProcessHandle {
    pid: u32,
}

#[cfg(all(unix, not(target_os = "linux")))]
impl ProcessHandle {
    fn open(pid: u32) -> Result<Self, String> {
        Ok(Self { pid })
    }

    fn hard_kill(&self, pid: u32) -> Result<(), String> {
        // SAFETY: the PID belongs to the child registered by this runtime.
        // This fallback is used only on non-Linux Unix platforms.
        let result = unsafe { libc::kill(self.pid as i32, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(format!("worker {pid} could not be killed: {error}"))
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct ProcessHandle;

#[cfg(not(any(unix, windows)))]
impl ProcessHandle {
    fn open(_pid: u32) -> Result<Self, String> {
        Err(String::from(
            "hard worker stop is unsupported on this platform",
        ))
    }

    fn hard_kill(&self, _pid: u32) -> Result<(), String> {
        Err(String::from(
            "hard worker stop is unsupported on this platform",
        ))
    }
}

fn hard_kill_worker(worker: &WorkerHandle) -> Result<(), String> {
    worker.hard_stopped.store(true, Ordering::SeqCst);
    let _previous = worker.hard_stop_signal.send_replace(true);
    worker.process.hard_kill(worker.pid)
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
    let staging_preparation =
        effect_runtime.staging_preparation_for_action(&root, &requested_path, operation);
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
    if abort.is_aborted() {
        return Ok(WorkspaceActionHostOutcome::Stopped);
    }
    let prepared = match effect_runtime
        .prepare_worker(abort, staging_preparation)
        .await
    {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return Ok(WorkspaceActionHostOutcome::Stopped),
        Err(_error) if effect_runtime.is_closing() => {
            return Ok(WorkspaceActionHostOutcome::Stopped);
        }
        Err(error) => return Err(error),
    };
    let mut tracker = ActionEvaluationTracker::new();
    let claim_scope = executions.task_claim_scope(TaskClaimKind::Action).await;
    if abort.is_aborted() || effect_runtime.is_closing() {
        drop(claim_scope);
        effect_runtime
            .stop_prepared(prepared)
            .await
            .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
        return Ok(WorkspaceActionHostOutcome::Stopped);
    }
    let claim = start_workspace_action(store, &mut tracker, command).await;
    drop(claim_scope);
    let started = match claim {
        Ok(ActionClaimOutcome::Started(started)) => started,
        Ok(ActionClaimOutcome::NotStarted(ActionNotStarted::TaskTerminal)) => {
            effect_runtime
                .stop_prepared(prepared)
                .await
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return match store.load_task(task).await? {
                Some(record) => Ok(WorkspaceActionHostOutcome::TaskTerminal {
                    task,
                    progress: record.task.progress,
                }),
                None => Ok(WorkspaceActionHostOutcome::MissingTask { task }),
            };
        }
        Ok(ActionClaimOutcome::NotStarted(ActionNotStarted::ExecutionSealed)) => {
            effect_runtime
                .stop_prepared(prepared)
                .await
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return Ok(WorkspaceActionHostOutcome::ExecutionSealed { delegation });
        }
        Ok(ActionClaimOutcome::NotStarted(reason)) => {
            effect_runtime
                .stop_prepared(prepared)
                .await
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return Ok(WorkspaceActionHostOutcome::NotStarted(reason));
        }
        Err(error) => {
            effect_runtime
                .stop_prepared(prepared)
                .await
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return Err(error.into());
        }
    };
    let attempt = started.attempt();
    let effect = match effect_runtime
        .execute_prepared(&started, abort, prepared)
        .await
    {
        Ok(TaskEffectExecution::Completed(effect)) => effect,
        Ok(TaskEffectExecution::Aborted) => return Ok(WorkspaceActionHostOutcome::Stopped),
        Err(_error) if effect_runtime.is_closing() => {
            return Ok(WorkspaceActionHostOutcome::Stopped);
        }
        Err(error) => return Err(error),
    };
    #[cfg(feature = "test-support")]
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

#[cfg(all(test, feature = "test-support"))]
mod supervisor_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use ene_action::{OperationKind, RealTargetRef, WorkspaceEffectStagingPause, WorkspaceRoot};
    use tokio::time::timeout;

    use super::{
        DispatchAbort, TaskEffectExecution, TaskEffectRuntime, WorkspaceActionHostError,
        effect_worker_command_at,
    };
    use crate::staging_cleanup::{
        StagingCleanupPause, StagingLeaseOptions, cleanup_staging_lease, prepare_staging_lease,
    };

    #[test]
    fn worker_path_prefers_a_sibling_and_falls_back_to_the_host_mode() {
        let directory = tempfile::tempdir().expect("path fixture");
        let file_name = format!("ene-action-worker{}", std::env::consts::EXE_SUFFIX);
        let host = directory
            .path()
            .join(format!("ene-core{}", std::env::consts::EXE_SUFFIX));
        let sibling = directory.path().join(&file_name);
        std::fs::write(&host, b"host").expect("host fixture");
        std::fs::write(&sibling, b"worker").expect("worker fixture");
        let command = effect_worker_command_at(&host, &file_name, true);
        assert_eq!(command.executable, sibling);
        std::fs::remove_file(&sibling).expect("remove sibling fixture");
        let command = effect_worker_command_at(&host, &file_name, true);
        assert_eq!(command.executable, host);
        assert_eq!(command.args, vec![String::from("workspace-effect-worker")]);
        let release_command = effect_worker_command_at(&host, &file_name, false);
        assert_eq!(release_command.executable, host.with_file_name(&file_name));
    }

    #[tokio::test]
    async fn stale_worker_protocol_is_rejected_before_any_effect() {
        let marker_dir = tempfile::tempdir().expect("marker directory");
        let executable = std::env::current_exe().expect("test executable");
        let runtime = TaskEffectRuntime::for_test_command(
            executable,
            vec![
                String::from("--ignored"),
                String::from("--exact"),
                String::from("action::supervisor_tests::parked_worker_fixture"),
                String::from("--nocapture"),
            ],
            vec![
                (
                    String::from("ENE_TEST_WORKER_MARKERS"),
                    marker_dir.path().to_string_lossy().into_owned(),
                ),
                (
                    String::from("ENE_TEST_WORKER_GENERATION"),
                    String::from("0"),
                ),
            ],
        );
        let result = runtime
            .prepare_worker(&DispatchAbort::default(), None)
            .await;
        assert!(matches!(
            result,
            Err(WorkspaceActionHostError::WorkerProtocolMismatch {
                expected: 2,
                actual: Some(0)
            })
        ));
        assert_eq!(runtime.live_workers_for_tests(), 0);
        assert_eq!(
            std::fs::read_dir(marker_dir.path())
                .expect("marker directory")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn malformed_worker_handshake_is_rejected_before_any_effect() {
        let marker_dir = tempfile::tempdir().expect("marker directory");
        let executable = std::env::current_exe().expect("test executable");
        let runtime = TaskEffectRuntime::for_test_command(
            executable,
            vec![
                String::from("--ignored"),
                String::from("--exact"),
                String::from("action::supervisor_tests::parked_worker_fixture"),
                String::from("--nocapture"),
            ],
            vec![
                (
                    String::from("ENE_TEST_WORKER_MARKERS"),
                    marker_dir.path().to_string_lossy().into_owned(),
                ),
                (
                    String::from("ENE_TEST_WORKER_MALFORMED_HANDSHAKE"),
                    String::from("1"),
                ),
            ],
        );
        let result = runtime
            .prepare_worker(&DispatchAbort::default(), None)
            .await;
        assert!(matches!(
            result,
            Err(WorkspaceActionHostError::WorkerHandshakeFailed { .. })
        ));
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[test]
    #[ignore = "subprocess fixture for worker supervisor tests"]
    fn parked_worker_fixture() {
        if std::env::var_os(crate::staging_cleanup::STAGING_HELPER_MODE_ENV).is_some() {
            crate::run_workspace_staging_helper();
            return;
        }
        use std::io::{Read, Write};

        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("protocol handshake");
        let handshake = serde_json::from_str::<serde_json::Value>(&input).expect("handshake json");
        let generation = std::env::var("ENE_TEST_WORKER_GENERATION")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_else(|| handshake["generation"].as_u64().expect("generation"));
        let malformed = std::env::var_os("ENE_TEST_WORKER_MALFORMED_HANDSHAKE").is_some();
        let response = if malformed {
            String::from("{not-json")
        } else {
            serde_json::json!({ "generation": generation }).to_string()
        };
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{response}").expect("handshake response");
        stdout.flush().expect("handshake flush");
        if malformed || generation != handshake["generation"].as_u64().expect("generation") {
            return;
        }

        let marker_dir = std::env::var_os("ENE_TEST_WORKER_MARKERS").expect("marker directory");
        let marker_dir = PathBuf::from(marker_dir);
        std::fs::create_dir_all(&marker_dir).expect("marker directory");
        std::fs::write(
            marker_dir.join(format!("entered-{}", std::process::id())),
            b"entered",
        )
        .expect("marker write");
        let mut request = Vec::new();
        std::io::stdin()
            .read_to_end(&mut request)
            .expect("effect request");
        loop {
            std::thread::park();
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "test helper must report a missing test executable"
    )]
    fn fixture_runtime(marker_dir: &Path) -> TaskEffectRuntime {
        let executable = std::env::current_exe().expect("test executable");
        TaskEffectRuntime::for_test_command(
            executable,
            vec![
                String::from("--ignored"),
                String::from("--exact"),
                String::from("action::supervisor_tests::parked_worker_fixture"),
                String::from("--nocapture"),
            ],
            vec![(
                String::from("ENE_TEST_WORKER_MARKERS"),
                marker_dir.to_string_lossy().into_owned(),
            )],
        )
    }

    #[expect(
        clippy::expect_used,
        reason = "test helper must fail with a bounded barrier diagnostic"
    )]
    async fn wait_for_markers(directory: &Path, count: usize) {
        timeout(Duration::from_secs(15), async {
            loop {
                let found = std::fs::read_dir(directory)
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .filter(|entry| {
                                entry.file_name().to_string_lossy().starts_with("entered-")
                            })
                            .count()
                    })
                    .unwrap_or(0);
                if found >= count {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker markers must arrive");
    }

    fn started(
        root: &WorkspaceRoot,
        name: &str,
        content: Option<Vec<u8>>,
    ) -> ene_action::StartedWorkspaceAction {
        let target = root.as_path().join(name);
        ene_action::StartedWorkspaceAction::for_test(
            root.clone(),
            RealTargetRef::from_canonical_path(target.to_string_lossy().into_owned()),
            OperationKind::Create,
            content,
        )
    }

    #[tokio::test]
    async fn stdin_backpressure_is_hard_stopped_and_reaped() {
        let directory = tempfile::tempdir().expect("test directory");
        let markers = directory.path().join("markers");
        std::fs::create_dir_all(&markers).expect("marker directory");
        let mut runtime = fixture_runtime(&markers);
        runtime.set_test_stdin_write_marker(markers.join("entered-write"));
        let runtime = Arc::new(runtime);
        let workspace = tempfile::tempdir().expect("workspace");
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let started = started(&root, "blocked.txt", Some(vec![b'x'; 8 * 1024 * 1024]));
        let abort = DispatchAbort::default();
        let execution = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move { runtime.execute(&started, &abort).await })
        };
        let mut execution = execution;
        tokio::select! {
            result = &mut execution => panic!("fixture execution ended before the barrier: {result:?}"),
            () = wait_for_markers(&markers, 2) => {}
        }
        let stopped = timeout(Duration::from_secs(15), async {
            let (stop, result) = tokio::join!(runtime.terminate_and_join(), execution);
            (stop, result)
        })
        .await
        .expect("hard stop must be bounded");
        assert!(stopped.0.is_ok());
        assert!(matches!(
            stopped.1.expect("runner joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[tokio::test]
    async fn one_hard_stop_reaps_all_registered_workers() {
        let directory = tempfile::tempdir().expect("test directory");
        let markers = directory.path().join("markers");
        let runtime = Arc::new(fixture_runtime(&markers));
        let workspace = tempfile::tempdir().expect("workspace");
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let first = started(&root, "first.txt", Some(vec![b'a'; 1024]));
        let second = started(&root, "second.txt", Some(vec![b'b'; 1024]));
        let first_abort = DispatchAbort::default();
        let second_abort = DispatchAbort::default();
        let first_task = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move { runtime.execute(&first, &first_abort).await })
        };
        let second_task = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move { runtime.execute(&second, &second_abort).await })
        };
        let mut first_task = first_task;
        let mut second_task = second_task;
        tokio::select! {
            result = &mut first_task => panic!("first fixture execution ended before the barrier: {result:?}"),
            result = &mut second_task => panic!("second fixture execution ended before the barrier: {result:?}"),
            () = wait_for_markers(&markers, 2) => {}
        }
        let stopped = timeout(Duration::from_secs(15), async {
            let (stop, first, second) =
                tokio::join!(runtime.terminate_and_join(), first_task, second_task);
            (stop, first, second)
        })
        .await
        .expect("all workers must stop");
        assert!(stopped.0.is_ok());
        assert!(matches!(
            stopped.1.expect("first joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert!(matches!(
            stopped.2.expect("second joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[tokio::test]
    async fn hard_stop_cleans_a_staging_directory_before_join() {
        let directory = tempfile::tempdir().expect("test directory");
        let workspace = tempfile::tempdir().expect("workspace");
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let entered = directory.path().join("staging-entered");
        let release = directory.path().join("staging-release");
        let runtime = TaskEffectRuntime::new();
        runtime.set_test_staging_pause(Some(WorkspaceEffectStagingPause {
            entered: entered.clone(),
            release,
        }));
        let started = started(&root, "staged.txt", Some(b"secret body".to_vec()));
        let abort = DispatchAbort::default();
        let execution = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.execute(&started, &abort).await }
        });
        let mut execution = execution;
        tokio::select! {
            result = &mut execution => panic!("create staging worker ended before the barrier: {result:?}"),
            result = timeout(Duration::from_secs(15), async {
                while !entered.exists() {
                    tokio::task::yield_now().await;
                }
            }) => {
                if result.is_err() {
                    panic!("staging barrier must be reached");
                }
            }
        }
        let stopped = timeout(Duration::from_secs(15), async {
            let (stop, result) = tokio::join!(runtime.terminate_and_join(), execution);
            (stop, result)
        })
        .await
        .expect("staging hard stop must be bounded");
        assert!(stopped.0.is_ok());
        assert!(matches!(
            stopped.1.expect("runner joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert!(!workspace.path().join("staged.txt").exists());
        assert_eq!(
            std::fs::read_dir(workspace.path().join(".ene-action-staging"))
                .expect("staging root")
                .count(),
            0
        );
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[tokio::test]
    async fn stalled_staging_preparation_is_hard_stopped_without_late_mutation() {
        let directory = tempfile::tempdir().expect("test directory");
        let workspace = tempfile::tempdir().expect("workspace");
        let markers = directory.path().join("markers");
        std::fs::create_dir_all(&markers).expect("marker directory");
        let entered = directory.path().join("prepare-entered");
        let release = directory.path().join("prepare-release");
        let canary = directory.path().join("prepare-canary");
        let runtime = fixture_runtime(&markers);
        runtime.set_test_staging_helper_pause(
            "prepare",
            entered.clone(),
            release.clone(),
            Some(canary.clone()),
        );
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let started = started(&root, "stalled-prepare.txt", Some(b"blocked".to_vec()));
        let preparation = runtime
            .staging_preparation_for_started(&started)
            .expect("staging preparation");
        let staging_path = preparation.path.clone();
        let abort = DispatchAbort::default();
        let execution = tokio::spawn({
            let runtime = runtime.clone();
            async move {
                runtime
                    .execute_with_preparation(&started, &abort, Some(preparation))
                    .await
            }
        });
        let execution = execution;
        timeout(Duration::from_secs(10), async {
            while !entered.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("staging preparation must enter its barrier");
        let stopped = timeout(Duration::from_secs(10), async {
            let (stop, result) = tokio::join!(runtime.terminate_and_join(), execution);
            (stop, result)
        })
        .await
        .expect("hard stop must remain bounded");
        assert!(stopped.0.is_err());
        assert!(matches!(
            stopped.1.expect("runner joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert_eq!(runtime.live_worker_processes_for_tests(), 0);
        assert_eq!(runtime.pending_staging_obligations_for_tests(), 1);
        assert!(staging_path.is_dir());
        std::fs::write(&release, b"release").expect("release condition");
        assert!(
            !canary.exists(),
            "a killed helper must not resume after return"
        );
        assert!(staging_path.is_dir());
        assert_eq!(
            std::fs::read_dir(workspace.path().join(".ene-action-staging"))
                .expect("staging root")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn stalled_cleanup_is_killed_reaped_and_keeps_an_unresolved_obligation() {
        let directory = tempfile::tempdir().expect("test directory");
        let workspace = tempfile::tempdir().expect("workspace");
        let entered = directory.path().join("cleanup-entered");
        let release = directory.path().join("cleanup-release");
        let canary = directory.path().join("cleanup-canary");
        let runtime = TaskEffectRuntime::new();
        runtime.set_test_staging_helper_pause(
            "cleanup",
            entered.clone(),
            release.clone(),
            Some(canary.clone()),
        );
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let started = started(&root, "stalled-cleanup.txt", Some(b"private body".to_vec()));
        let abort = DispatchAbort::default();
        let execution = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.execute(&started, &abort).await }
        });
        let execution = execution;
        timeout(Duration::from_secs(10), async {
            while !entered.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cleanup must enter its barrier");
        runtime.set_test_cleanup_failure(true);
        let stopped = timeout(Duration::from_secs(10), async {
            let (stop, result) = tokio::join!(runtime.terminate_and_join(), execution);
            (stop, result)
        })
        .await
        .expect("hard stop must remain bounded");
        assert!(stopped.0.is_err());
        assert!(matches!(
            stopped.1.expect("runner joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert_eq!(runtime.live_worker_processes_for_tests(), 0);
        assert_eq!(runtime.pending_staging_obligations_for_tests(), 1);
        std::fs::write(&release, b"release").expect("release condition");
        assert!(!canary.exists(), "a killed cleanup helper must not resume");
        assert!(workspace.path().join("stalled-cleanup.txt").exists());
    }

    #[tokio::test]
    async fn staging_collision_never_deletes_unowned_content() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path().join(".ene-action-staging");
        let existing = root.join("existing");
        std::fs::create_dir_all(&existing).expect("staging fixture");
        let sentinel = existing.join("sentinel");
        std::fs::write(&sentinel, b"keep").expect("sentinel write");
        let workspace_root =
            WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace root");
        let result =
            prepare_staging_lease(&existing, &workspace_root, StagingLeaseOptions::default()).await;
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&sentinel).expect("sentinel survives"),
            b"keep"
        );
    }

    #[tokio::test]
    async fn effect_startup_never_deletes_an_unowned_attempt_directory() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root =
            WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace root");
        let started = started(&root, "collision.txt", Some(b"blocked".to_vec()));
        let runtime = TaskEffectRuntime::new();
        let preparation = runtime
            .staging_preparation_for_started(&started)
            .expect("staging preparation");
        let collision = preparation.path.clone();
        std::fs::create_dir_all(&collision).expect("staging collision");
        let sentinel = collision.join("sentinel");
        std::fs::write(&sentinel, b"keep").expect("sentinel write");

        let result = runtime
            .execute_with_preparation(&started, &DispatchAbort::default(), Some(preparation))
            .await;

        assert!(matches!(
            result,
            Err(WorkspaceActionHostError::EffectUnavailable { .. })
        ));
        assert_eq!(
            std::fs::read(&sentinel).expect("sentinel survives"),
            b"keep"
        );
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[tokio::test]
    async fn rename_away_cleanup_uses_the_owned_directory_object() {
        let workspace = tempfile::tempdir().expect("workspace");
        let workspace_root =
            WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace root");
        let staging_path = workspace_root
            .as_path()
            .join(".ene-action-staging")
            .join("attempt");
        let staging = prepare_staging_lease(
            &staging_path,
            &workspace_root,
            StagingLeaseOptions::default(),
        )
        .await
        .expect("owned staging");
        std::fs::write(staging_path.join("target-body"), b"target-bearing content")
            .expect("owned content");
        let moved = staging_path.with_file_name("moved");
        std::fs::rename(&staging_path, &moved).expect("move owned staging");
        assert!(!staging_path.exists());
        std::fs::create_dir(&staging_path).expect("replacement staging");
        let sentinel = staging_path.join("sentinel");
        std::fs::write(&sentinel, b"keep").expect("sentinel write");

        cleanup_staging_lease(staging)
            .await
            .expect("renamed owned staging cleanup");

        assert!(!moved.exists());
        assert_eq!(
            std::fs::read(&sentinel).expect("replacement survives"),
            b"keep"
        );
    }

    #[tokio::test]
    async fn replacement_after_validation_never_redirects_owned_cleanup() {
        let workspace = tempfile::tempdir().expect("workspace");
        let barriers = tempfile::tempdir().expect("barriers");
        let entered = barriers.path().join("validated");
        let release = barriers.path().join("release");
        let workspace_root =
            WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace root");
        let staging_path = workspace_root
            .as_path()
            .join(".ene-action-staging")
            .join("attempt");
        let runtime = TaskEffectRuntime::new();
        runtime.set_test_cleanup_pause(Some(StagingCleanupPause {
            entered: entered.clone(),
            release: release.clone(),
        }));
        let staging = prepare_staging_lease(
            &staging_path,
            &workspace_root,
            runtime.staging_lease_options(),
        )
        .await
        .expect("owned staging");
        std::fs::write(staging_path.join("target-body"), b"target-bearing content")
            .expect("owned content");
        let cleanup = tokio::spawn(cleanup_staging_lease(staging));
        timeout(Duration::from_secs(15), async {
            while !entered.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cleanup identity barrier");

        let moved = staging_path.with_file_name("moved");
        std::fs::rename(&staging_path, &moved).expect("move owned staging");
        std::fs::create_dir(&staging_path).expect("replacement staging");
        let sentinel = staging_path.join("sentinel");
        std::fs::write(&sentinel, b"keep").expect("sentinel write");
        std::fs::write(&release, b"release").expect("release cleanup");

        cleanup
            .await
            .expect("cleanup task joins")
            .expect("owned object cleanup");
        assert!(!moved.exists());
        assert_eq!(
            std::fs::read(&sentinel).expect("replacement survives"),
            b"keep"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_staging_root_is_rejected_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let sentinel = outside.path().join("keep.txt");
        std::fs::write(&sentinel, b"keep").expect("sentinel write");
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let staging_root = workspace.path().join(".ene-action-staging");
        symlink(outside.path(), &staging_root).expect("staging symlink");
        let started = started(&root, "escape.txt", Some(b"blocked".to_vec()));
        let runtime = TaskEffectRuntime::new();
        let result = runtime.execute(&started, &DispatchAbort::default()).await;
        assert!(matches!(
            result,
            Err(WorkspaceActionHostError::EffectUnavailable { .. })
        ));
        assert!(!workspace.path().join("escape.txt").exists());
        assert_eq!(
            std::fs::read(&sentinel).expect("sentinel survives"),
            b"keep"
        );
        assert_eq!(
            std::fs::read_dir(outside.path()).expect("outside").count(),
            1
        );
        assert!(std::fs::symlink_metadata(&staging_root).is_ok());
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[tokio::test]
    async fn cleanup_retry_timeout_kills_and_reaps_the_unregistered_helper() {
        let directory = tempfile::tempdir().expect("test directory");
        let workspace = tempfile::tempdir().expect("workspace");
        let markers = directory.path().join("markers");
        std::fs::create_dir_all(&markers).expect("marker directory");
        let entered = directory.path().join("retry-cleanup-entered");
        let release = directory.path().join("retry-cleanup-release");
        let canary = directory.path().join("retry-cleanup-canary");
        let mut runtime = fixture_runtime(&markers);
        runtime.command.staging_executable = None;
        runtime.set_test_staging_helper_pause(
            "cleanup",
            entered.clone(),
            release.clone(),
            Some(canary.clone()),
        );

        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let preparation =
            runtime.new_staging_preparation(root.clone(), root.as_path().to_path_buf());
        let options = StagingLeaseOptions {
            ownership_token: Some(preparation.ownership_token.clone()),
            ..StagingLeaseOptions::default()
        };
        let lease = prepare_staging_lease(&preparation.path, &root, options)
            .await
            .expect("owned staging");
        drop(lease);
        let obligation = super::StagingCleanupObligation {
            preparation: preparation.clone(),
            identity: None,
            control: None,
            prepared: true,
        };

        let result = runtime
            .cleanup_obligation_by_path_with_timeout(&obligation, Duration::from_millis(100))
            .await;

        assert!(matches!(
            result,
            Err(ref reason) if reason.contains("staging obligation cleanup timed out")
        ));
        assert!(entered.exists(), "cleanup retry must enter its barrier");
        assert!(
            preparation.path.is_dir(),
            "timed-out cleanup remains an unresolved obligation"
        );
        std::fs::write(&release, b"release").expect("release condition");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !canary.exists(),
            "the killed and reaped retry helper cannot mutate after return"
        );
    }

    #[tokio::test]
    async fn hard_stop_cleans_edit_staging_without_replacing_the_target() {
        let directory = tempfile::tempdir().expect("test directory");
        let workspace = tempfile::tempdir().expect("workspace");
        let target_path = workspace.path().join("edit.txt");
        std::fs::write(&target_path, b"before").expect("existing edit target");
        let root = WorkspaceRoot::open(&workspace.path().to_string_lossy()).expect("workspace");
        let entered = directory.path().join("edit-entered");
        let release = directory.path().join("edit-release");
        let runtime = TaskEffectRuntime::new();
        runtime.set_test_staging_pause(Some(WorkspaceEffectStagingPause {
            entered: entered.clone(),
            release,
        }));
        let canonical_target = std::fs::canonicalize(&target_path).expect("canonical edit target");
        let started = ene_action::StartedWorkspaceAction::for_test(
            root.clone(),
            RealTargetRef::from_canonical_path(canonical_target.to_string_lossy().into_owned()),
            OperationKind::Edit,
            Some(b"after".to_vec()),
        );
        let abort = DispatchAbort::default();
        let execution = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.execute(&started, &abort).await }
        });
        let mut execution = execution;
        tokio::select! {
            result = &mut execution => panic!("edit staging worker ended before the barrier: {result:?}"),
            result = timeout(Duration::from_secs(15), async {
                while !entered.exists() {
                    tokio::task::yield_now().await;
                }
            }) => {
                if result.is_err() {
                    panic!("staging barrier must be reached");
                }
            }
        }
        let stopped = timeout(Duration::from_secs(15), async {
            let (stop, result) = tokio::join!(runtime.terminate_and_join(), execution);
            (stop, result)
        })
        .await
        .expect("edit staging hard stop must be bounded");
        assert!(stopped.0.is_ok());
        assert!(matches!(
            stopped.1.expect("runner joins"),
            Ok(TaskEffectExecution::Aborted)
        ));
        assert_eq!(
            std::fs::read(&target_path).expect("edit target remains"),
            b"before"
        );
        assert_eq!(
            std::fs::read_dir(workspace.path().join(".ene-action-staging"))
                .expect("staging root")
                .count(),
            0
        );
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }
}
