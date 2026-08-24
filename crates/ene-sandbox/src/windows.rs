//! Windows sandbox: Job Object with process/memory/CPU limits.
//!
//! The child is spawned `CREATE_SUSPENDED`, assigned to a job that kills the
//! whole tree on close, then resumed — so descendants cannot escape the
//! limits and the host's drop of the job terminates every sandboxed process.
//! Restricted-token / AppContainer hardening (no direct network capability,
//! low integrity) is the documented next step: it requires a custom
//! `CreateProcessAsUserW` spawn path and is not yet wired here.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::ptr;

use crate::error::SandboxError;
use crate::spec::SandboxSpec;

const CREATE_SUSPENDED: u32 = 0x0000_0004;
const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
const THREAD_SUSPEND_RESUME: u32 = 0x0000_0002;
const INVALID_HANDLE_VALUE: isize = -1;

const JOB_OBJECT_LIMIT_PROCESS_TIME: u32 = 0x0000_0002;
const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;

type Handle = *mut core::ffi::c_void;
type Bool = i32;

#[repr(C)]
struct LargeInteger {
    value: i64,
}

#[repr(C)]
struct IoCounters {
    read_operations: u64,
    write_operations: u64,
    other_operations: u64,
    read_transfers: u64,
    write_transfers: u64,
    other_transfers: u64,
}

#[repr(C)]
struct BasicLimitInformation {
    per_process_user_time_limit: LargeInteger,
    per_job_user_time_limit: LargeInteger,
    limit_flags: u32,
    _pad1: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    _pad2: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
struct ExtendedLimitInformation {
    basic: BasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[repr(C)]
#[derive(Default)]
struct ThreadEntry32 {
    size: u32,
    usage: u32,
    thread_id: u32,
    owner_process_id: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *mut core::ffi::c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        class: i32,
        info: *mut core::ffi::c_void,
        length: u32,
    ) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn OpenThread(access: u32, inherit: Bool, thread_id: u32) -> Handle;
    fn ResumeThread(thread: Handle) -> u32;
    fn CloseHandle(handle: Handle) -> Bool;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
    fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
    fn GetLastError() -> u32;
}

fn win_error(operation: &str) -> SandboxError {
    // SAFETY: GetLastError is a plain read of the thread-local error slot.
    let code = unsafe { GetLastError() };
    SandboxError::Windows {
        code,
        message: operation.to_string(),
    }
}

/// Sets `CREATE_SUSPENDED` on the command so [`attach`] can assign the job
/// before the child runs any code.
pub fn prepare_command(
    cmd: &mut std::process::Command,
    spec: &SandboxSpec,
) -> Result<(), SandboxError> {
    if !spec.is_enforced() {
        return Ok(());
    }
    cmd.creation_flags(CREATE_SUSPENDED);
    Ok(())
}

/// Attaches a just-spawned (suspended) child to its sandbox job and resumes
/// it. The returned guard keeps the job alive; dropping it closes the job,
/// which kills every process in the tree.
pub fn attach(
    child: &mut std::process::Child,
    spec: &SandboxSpec,
) -> Result<JobGuard, SandboxError> {
    let job = create_job(spec)?;
    let process = child.as_raw_handle();
    // SAFETY: AssignProcessToJobObject takes a valid process handle (the
    // live child) and the job handle we just created.
    if unsafe { AssignProcessToJobObject(job, process) } == 0 {
        let error = win_error("AssignProcessToJobObject");
        // SAFETY: closing a handle we own.
        unsafe {
            CloseHandle(job);
        }
        return Err(error);
    }
    let thread_id = primary_thread_id(process)?;
    // SAFETY: OpenThread with THREAD_SUSPEND_RESUME on the child's primary
    // thread id (fetched from the toolhelp snapshot).
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() || thread as isize == INVALID_HANDLE_VALUE {
        // Extremely unlikely after a successful spawn; failing closed here
        // would orphan the child, so resume via the job is not possible —
        // report the error (the host kills the suspended child).
        return Err(win_error("OpenThread(primary)"));
    }
    // SAFETY: ResumeThread on the valid suspended primary thread.
    let resume = unsafe { ResumeThread(thread) };
    // SAFETY: closing the thread handle.
    unsafe {
        CloseHandle(thread);
    }
    if resume == u32::MAX {
        return Err(win_error("ResumeThread"));
    }
    Ok(JobGuard { job })
}

/// Owns the sandbox job; the job has `KILL_ON_JOB_CLOSE`, so dropping it
/// terminates the whole sandboxed tree.
#[derive(Debug)]
pub struct JobGuard {
    job: Handle,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        // SAFETY: closing the job handle we own; KILL_ON_JOB_CLOSE then
        // terminates all processes in the job.
        unsafe {
            CloseHandle(self.job);
        }
    }
}

fn create_job(spec: &SandboxSpec) -> Result<Handle, SandboxError> {
    // SAFETY: CreateJobObjectW with a null name creates an unnamed job.
    let job = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
    if job.is_null() || job as isize == INVALID_HANDLE_VALUE {
        return Err(win_error("CreateJobObjectW"));
    }
    let mut info = ExtendedLimitInformation {
        basic: BasicLimitInformation {
            per_process_user_time_limit: LargeInteger { value: 0 },
            per_job_user_time_limit: LargeInteger { value: 0 },
            limit_flags: 0,
            _pad1: 0,
            minimum_working_set_size: 0,
            maximum_working_set_size: 0,
            active_process_limit: 0,
            _pad2: 0,
            affinity: 0,
            priority_class: 0,
            scheduling_class: 0,
        },
        io_info: IoCounters {
            read_operations: 0,
            write_operations: 0,
            other_operations: 0,
            read_transfers: 0,
            write_transfers: 0,
            other_transfers: 0,
        },
        process_memory_limit: 0,
        job_memory_limit: 0,
        peak_process_memory_used: 0,
        peak_job_memory_used: 0,
    };
    info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if spec.limits.max_processes > 0 {
        info.basic.limit_flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.basic.active_process_limit =
            u32::try_from(spec.limits.max_processes).unwrap_or(u32::MAX);
    }
    if spec.limits.max_rss_bytes > 0 {
        info.basic.limit_flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.process_memory_limit =
            usize::try_from(spec.limits.max_rss_bytes).unwrap_or(usize::MAX);
    }
    if spec.limits.max_cpu_seconds > 0 {
        info.basic.limit_flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
        info.basic.per_process_user_time_limit.value =
            i64::try_from(spec.limits.max_cpu_seconds.saturating_mul(10_000_000))
                .unwrap_or(i64::MAX);
    }
    // SAFETY: SetInformationJobObject copies the struct synchronously; the
    // pointer is valid for the call.
    if unsafe {
        SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            (&raw mut info).cast(),
            std::mem::size_of::<ExtendedLimitInformation>() as u32,
        )
    } == 0
    {
        let error = win_error("SetInformationJobObject");
        // SAFETY: closing the handle we own.
        unsafe {
            CloseHandle(job);
        }
        return Err(error);
    }
    Ok(job)
}

fn primary_thread_id(process: Handle) -> Result<u32, SandboxError> {
    // SAFETY: CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD) takes a pid; 0
    // means "all processes" (needed because the child pid is not known to be
    // the primary thread id).
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot.is_null() || snapshot as isize == INVALID_HANDLE_VALUE {
        return Err(win_error("CreateToolhelp32Snapshot"));
    }
    let pid = process_id(process)?;
    let mut entry = ThreadEntry32 {
        size: std::mem::size_of::<ThreadEntry32>() as u32,
        ..ThreadEntry32::default()
    };
    // SAFETY: Thread32First/Next iterate the snapshot; the entry is valid
    // for the iteration.
    let mut found = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
    while found {
        if entry.owner_process_id == pid {
            // SAFETY: closing the snapshot handle.
            unsafe {
                CloseHandle(snapshot);
            }
            return Ok(entry.thread_id);
        }
        // SAFETY: Thread32Next advances the valid snapshot iterator into the initialized entry.
        found = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
    }
    // SAFETY: closing the snapshot handle.
    unsafe {
        CloseHandle(snapshot);
    }
    Err(SandboxError::Windows {
        code: 0,
        message: "primary thread not found".to_string(),
    })
}

fn process_id(process: Handle) -> Result<u32, SandboxError> {
    // SAFETY: GetProcessId reads the pid from a valid process handle.
    let pid = unsafe { GetProcessId(process) };
    if pid == 0 {
        Err(win_error("GetProcessId"))
    } else {
        Ok(pid)
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcessId(process: Handle) -> u32;
}
