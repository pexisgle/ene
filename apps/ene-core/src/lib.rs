pub mod action;
pub mod conn;
#[cfg(windows)]
pub mod conn_pipe;
pub mod deletion;
pub mod dialogue;
pub mod host_control;
pub mod host_lock;
mod pairing_delivery;
pub mod presentation;
pub mod serve;
pub mod setup;
pub mod targeted_deletion;
pub mod task_agent;
pub mod task_control;
pub mod task_run;
pub mod transient_erasure;
pub mod usage;

use std::sync::{Mutex as StdMutex, MutexGuard};

pub(crate) fn lock_unpoison<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
