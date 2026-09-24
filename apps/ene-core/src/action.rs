use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

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
    workers: Arc<StdMutex<WorkerRegistry>>,
    #[cfg(feature = "test-support")]
    test_staging_pause: Arc<StdMutex<Option<ene_action::WorkspaceEffectStagingPause>>>,
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
    staging_directory: Option<PathBuf>,
    hard_stopped: AtomicBool,
    hard_stop_signal: tokio::sync::watch::Sender<bool>,
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

impl TaskEffectRuntime {
    pub(crate) fn new() -> Self {
        Self::with_command(effect_worker_command())
    }

    fn with_command(command: EffectWorkerCommand) -> Self {
        Self {
            command,
            active: Arc::new(tokio::sync::RwLock::new(())),
            workers: Arc::new(StdMutex::new(WorkerRegistry::default())),
            #[cfg(feature = "test-support")]
            test_staging_pause: Arc::new(StdMutex::new(None)),
            #[cfg(feature = "test-support")]
            cooperative_abort: true,
        }
    }

    pub(crate) fn ensure_available(&self) -> Result<(), WorkspaceActionHostError> {
        if worker_executable_available(&self.command.executable) {
            return Ok(());
        }
        Err(WorkspaceActionHostError::EffectUnavailable {
            reason: format!(
                "worker executable is unavailable: {}",
                self.command.executable.display()
            ),
        })
    }

    async fn execute(
        &self,
        started: &StartedWorkspaceAction,
        abort: &DispatchAbort,
    ) -> Result<TaskEffectExecution, WorkspaceActionHostError> {
        let _active = self.active.read().await;
        if abort.is_aborted() || self.is_closing() {
            return Ok(TaskEffectExecution::Aborted);
        }

        let staging_directory = match started.operation() {
            OperationKind::Create | OperationKind::Edit => Some(self.staging_directory(started)),
            OperationKind::List | OperationKind::Read => None,
        };
        let request = WorkspaceEffectRequest {
            root: started.root().as_path().to_string_lossy().into_owned(),
            target: started.target().as_path().to_owned(),
            operation: started.operation().as_str().to_owned(),
            content: started.content().map(<[u8]>::to_vec),
            staging_directory: staging_directory
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
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
        if let Some(path) = staging_directory.as_ref()
            && let Err(reason) = prepare_staging_directory(path).await
        {
            return Err(WorkspaceActionHostError::EffectUnavailable { reason });
        }
        let mut command = self.process_command();
        let spawned = match self
            .spawn_registered(&mut command, staging_directory.clone())
            .await
        {
            Ok(spawned) => spawned,
            Err(reason) => {
                cleanup_staging(staging_directory)
                    .await
                    .map_err(
                        |cleanup_reason| WorkspaceActionHostError::EffectUnavailable {
                            reason: format!("{reason}; {cleanup_reason}"),
                        },
                    )?;
                return Err(WorkspaceActionHostError::EffectUnavailable { reason });
            }
        };
        let SpawnedWorker::Started { mut child, handle } = spawned else {
            cleanup_staging(staging_directory)
                .await
                .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return Ok(TaskEffectExecution::Aborted);
        };

        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                stop_child(&mut child).await;
                self.finish_worker(&handle);
                cleanup_staging(staging_directory)
                    .await
                    .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
                return Err(WorkspaceActionHostError::EffectUnavailable {
                    reason: String::from("worker stdin unavailable"),
                });
            }
        };
        let mut stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                stop_child(&mut child).await;
                self.finish_worker(&handle);
                cleanup_staging(staging_directory)
                    .await
                    .map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
                return Err(WorkspaceActionHostError::EffectUnavailable {
                    reason: String::from("worker stdout unavailable"),
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
            stop_child(&mut child).await;
            output.abort();
            drop(output.await);
            self.finish_worker(&handle);
            cleanup_staging(staging_directory)
                .await
                .map_err(
                    |cleanup_reason| WorkspaceActionHostError::EffectUnavailable {
                        reason: format!("{error}; {cleanup_reason}"),
                    },
                )?;
            return Err(WorkspaceActionHostError::EffectUnavailable {
                reason: error.to_string(),
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
            stop_child(&mut child).await;
        }

        let status = if stop_requested || write_error.is_some() {
            child.wait().await
        } else {
            tokio::select! {
                biased;
                () = self.wait_for_abort(abort) => {
                    stop_requested = true;
                    stop_child(&mut child).await;
                    child.wait().await
                }
                status = child.wait() => status,
            }
        };
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
        let cleanup = cleanup_staging(staging_directory).await;
        if cleanup.is_ok() {
            self.finish_worker(&handle);
        }

        if stop_requested || handle.hard_stopped.load(Ordering::SeqCst) {
            cleanup.map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
            return Ok(TaskEffectExecution::Aborted);
        }
        cleanup.map_err(|reason| WorkspaceActionHostError::EffectUnavailable { reason })?;
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

    fn process_command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.command.executable);
        command
            .args(&self.command.args)
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(feature = "test-support")]
        command.envs(self.command.envs.iter().map(|(key, value)| (key, value)));
        command
    }

    fn staging_directory(&self, started: &StartedWorkspaceAction) -> PathBuf {
        let target = Path::new(started.target().as_path());
        let parent = target.parent().unwrap_or_else(|| started.root().as_path());
        parent.join(".ene-action-staging").join(
            started
                .attempt()
                .as_raw()
                .as_uuid()
                .as_hyphenated()
                .to_string(),
        )
    }

    fn is_closing(&self) -> bool {
        crate::lock_unpoison(&self.workers).closing
    }

    async fn spawn_registered(
        &self,
        command: &mut tokio::process::Command,
        staging_directory: Option<PathBuf>,
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
                    staging_directory,
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

    fn finish_worker(&self, handle: &Arc<WorkerHandle>) {
        let mut registry = crate::lock_unpoison(&self.workers);
        registry.workers.remove(&handle.id);
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
        }
        let _active = self.active.write().await;
        for worker in &workers {
            let cleanup = match worker.staging_directory.clone() {
                Some(path) => cleanup_staging(Some(path)).await,
                None => Ok(()),
            };
            match cleanup {
                Ok(()) => self.finish_worker(worker),
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        let remaining = crate::lock_unpoison(&self.workers).workers.len();
        if remaining != 0 {
            return Err(format!("{remaining} worker(s) remained after hard stop"));
        }
        failure.map_or(Ok(()), Err)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn live_workers_for_tests(&self) -> usize {
        crate::lock_unpoison(&self.workers).workers.len()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn for_test_command(
        executable: PathBuf,
        args: Vec<String>,
        envs: Vec<(String, String)>,
    ) -> Self {
        let mut runtime = Self::with_command(EffectWorkerCommand {
            executable,
            args,
            envs,
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

    #[cfg(feature = "test-support")]
    fn test_staging_pause(&self) -> Option<ene_action::WorkspaceEffectStagingPause> {
        crate::lock_unpoison(&self.test_staging_pause).clone()
    }
}

async fn stop_child(child: &mut tokio::process::Child) {
    drop(child.kill().await);
    drop(child.wait().await);
}

async fn prepare_staging_directory(path: &Path) -> Result<(), String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let root = path
            .parent()
            .ok_or_else(|| String::from("staging directory has no parent"))?;
        let mut root_created = false;
        match std::fs::create_dir(root) {
            Ok(()) => root_created = true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata =
                    std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(String::from("staging root is not a real directory"));
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        if let Err(error) = std::fs::create_dir(&path) {
            if root_created {
                match std::fs::remove_dir(root) {
                    Ok(()) => {}
                    Err(cleanup_error)
                        if matches!(
                            cleanup_error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                        ) => {}
                    Err(cleanup_error) => {
                        return Err(format!(
                            "{error}; staging root cleanup failed: {cleanup_error}"
                        ));
                    }
                }
            }
            return if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err(format!("staging path already exists: {error}"))
            } else {
                Err(error.to_string())
            };
        }
        #[cfg(unix)]
        if let Err(error) = {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        } {
            let cleanup = std::fs::remove_dir(&path);
            if root_created {
                drop(std::fs::remove_dir(root));
            }
            return match cleanup {
                Ok(()) => Err(error.to_string()),
                Err(cleanup_error) => Err(format!(
                    "{error}; staging directory cleanup failed: {cleanup_error}"
                )),
            };
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("staging preparation task failed: {error}"))?
}

async fn cleanup_staging(path: Option<PathBuf>) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    tokio::task::spawn_blocking(move || {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            }
            Ok(metadata) if metadata.is_dir() => {
                std::fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
            }
            Ok(_) => return Err(String::from("staging path is no longer a directory")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        if let Some(root) = path.parent()
            && root
                .file_name()
                .is_some_and(|name| name == ".ene-action-staging")
        {
            match std::fs::remove_dir(root) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("staging cleanup task failed: {error}"))?
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
            stdin_write_marker: None,
        };
    }
    EffectWorkerCommand {
        executable: current.to_path_buf(),
        args: vec![String::from("workspace-effect-worker")],
        #[cfg(feature = "test-support")]
        envs: Vec::new(),
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
    effect_runtime.ensure_available()?;
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

    use super::{DispatchAbort, TaskEffectExecution, TaskEffectRuntime, effect_worker_command_at};

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

    #[test]
    #[ignore = "subprocess fixture for worker supervisor tests"]
    fn parked_worker_fixture() {
        let marker_dir = std::env::var_os("ENE_TEST_WORKER_MARKERS").expect("marker directory");
        let marker_dir = PathBuf::from(marker_dir);
        std::fs::create_dir_all(&marker_dir).expect("marker directory");
        std::fs::write(
            marker_dir.join(format!("entered-{}", std::process::id())),
            b"entered",
        )
        .expect("marker write");
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
        assert!(!workspace.path().join(".ene-action-staging").exists());
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }

    #[tokio::test]
    async fn staging_collision_never_deletes_unowned_content() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path().join(".ene-action-staging");
        let existing = root.join("existing");
        std::fs::create_dir_all(&existing).expect("staging fixture");
        let sentinel = existing.join("sentinel");
        std::fs::write(&sentinel, b"keep").expect("sentinel write");
        let result = super::prepare_staging_directory(&existing).await;
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&sentinel).expect("sentinel survives"),
            b"keep"
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
        assert!(!workspace.path().join(".ene-action-staging").exists());
        assert_eq!(runtime.live_workers_for_tests(), 0);
    }
}
