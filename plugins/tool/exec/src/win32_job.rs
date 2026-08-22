//! Windows Job Object wrapper for killing spawned process trees.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::ptr;

type Handle = *mut core::ffi::c_void;
type Bool = i32;

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const INVALID_HANDLE_VALUE: isize = -1;

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
    fn CloseHandle(handle: Handle) -> Bool;
}

pub struct JobGuard {
    job: Handle,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        // SAFETY: closing a job handle we own; KILL_ON_JOB_CLOSE terminates the tree.
        unsafe {
            CloseHandle(self.job);
        }
    }
}

/// Assign `child` to a job object that kills the process tree when the guard drops.
///
/// # Errors
///
/// Returns an error when Win32 job setup fails.
pub fn assign(child: &Child) -> Result<JobGuard, String> {
    // SAFETY: CreateJobObjectW with null name creates an unnamed job object.
    let job = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
    if job.is_null() || job as isize == INVALID_HANDLE_VALUE {
        return Err("CreateJobObjectW failed".to_owned());
    }
    let mut info = ExtendedLimitInformation {
        basic: BasicLimitInformation {
            per_process_user_time_limit: LargeInteger { value: 0 },
            per_job_user_time_limit: LargeInteger { value: 0 },
            limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
    let info_len = u32::try_from(std::mem::size_of::<ExtendedLimitInformation>())
        .map_err(|_| "job info size overflow".to_owned())?;
    // SAFETY: SetInformationJobObject reads the struct synchronously for this call.
    if unsafe {
        SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            (&raw mut info).cast(),
            info_len,
        )
    } == 0
    {
        // SAFETY: closing the handle we own after a failed setup.
        unsafe {
            CloseHandle(job);
        }
        return Err("SetInformationJobObject failed".to_owned());
    }
    let process = child.as_raw_handle();
    // SAFETY: AssignProcessToJobObject takes valid process and job handles.
    if unsafe { AssignProcessToJobObject(job, process) } == 0 {
        // SAFETY: closing the handle we own after a failed assignment.
        unsafe {
            CloseHandle(job);
        }
        return Err("AssignProcessToJobObject failed".to_owned());
    }
    Ok(JobGuard { job })
}
