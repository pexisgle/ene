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

/// Locks a `std` mutex, recovering from poisoning.
///
/// Poisoning only follows a panic inside a critical section; the mutex's own
/// map and queue operations cannot panic while holding the guard, so recovery
/// preserves the committed state, but callers may run callbacks under a guard
/// that panic and poison it (for example `ConnectionTable::note_closed`).
/// Recovery then restores the maps as committed before the callback ran.
pub(crate) fn lock_unpoison<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
