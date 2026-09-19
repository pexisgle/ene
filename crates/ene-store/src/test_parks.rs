//! Test-only deterministic parks for production mutation boundaries.
//!
//! Each park is first-waiter-only: the first armed caller pauses after
//! signalling entry and before any SQLite lock is taken; later callers pass
//! through. That lets a test hold one in-flight production path while another
//! driver finishes the operation, then release the parked caller to observe
//! a stale no-op. Production builds never compile this module.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tokio::sync::Semaphore;

/// One first-waiter park used by a production async mutation entry.
#[derive(Debug)]
pub(crate) struct TestPark {
    armed: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}

impl Default for TestPark {
    fn default() -> Self {
        Self {
            armed: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

impl TestPark {
    pub(crate) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Pauses only if this park was armed and this is the first waiter.
    pub(crate) async fn pause_if_armed(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        self.entered.add_permits(1);
        let Ok(permit) = self.release.acquire().await else {
            return;
        };
        permit.forget();
    }

    pub(crate) async fn wait_entered(&self) {
        let Ok(permit) = self.entered.acquire().await else {
            return;
        };
        permit.forget();
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

/// Parks a started Targeted Deletion `spawn_blocking` section.
///
/// Unlike [`TestPark`], this waits on the blocking thread: aborting the
/// awaiting async task cannot skip past a started SQLite closure.
#[derive(Debug)]
pub(crate) struct BlockingPark {
    armed: AtomicBool,
    entered: AtomicBool,
    released: AtomicBool,
    mutex: Mutex<()>,
    cv: std::sync::Condvar,
    entered_notify: tokio::sync::Notify,
}

impl Default for BlockingPark {
    fn default() -> Self {
        Self {
            armed: AtomicBool::new(false),
            entered: AtomicBool::new(false),
            released: AtomicBool::new(false),
            mutex: Mutex::new(()),
            cv: std::sync::Condvar::new(),
            entered_notify: tokio::sync::Notify::new(),
        }
    }
}

impl BlockingPark {
    pub(crate) fn arm(&self) {
        self.entered.store(false, Ordering::SeqCst);
        self.released.store(false, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Pauses only if this park was armed and this is the first waiter.
    pub(crate) fn pause_blocking_if_armed(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        self.entered.store(true, Ordering::SeqCst);
        self.entered_notify.notify_waiters();
        let mut guard = self
            .mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !self.released.load(Ordering::SeqCst) {
            guard = self
                .cv
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    pub(crate) async fn wait_entered(&self) {
        let notified = self.entered_notify.notified();
        if self.entered.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }

    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.cv.notify_all();
    }
}

/// Parks for the mutation windows the Stage 6 race regressions fix.
#[derive(Debug, Default)]
pub(crate) struct TestParks {
    pub(crate) observation_write: TestPark,
    pub(crate) erasure_mutation: TestPark,
    pub(crate) device_auth_file: TestPark,
    pub(crate) client_demand: TestPark,
    pub(crate) learning_formation: TestPark,
    pub(crate) host_transient_queue: TestPark,
    pub(crate) learning_take: TestPark,
    pub(crate) host_transient_verified_record: TestPark,
    pub(crate) deletion_finalizing: TestPark,
    pub(crate) learning_pin_queue: TestPark,
    pub(crate) host_transient_arrival_publish: TestPark,
    pub(crate) fail_host_transient_arrival: AtomicBool,
    pub(crate) fail_host_transient_arrival_sticky: AtomicBool,
    pub(crate) fail_deletion_material: Mutex<Option<ene_preservation::DeletionOperationId>>,
    pub(crate) host_transient_arrival_attempts: AtomicU64,
    /// Started Targeted Deletion `spawn_blocking` sections. Observation only.
    pub(crate) deletion_blocking_live: AtomicUsize,
    pub(crate) deletion_blocking: BlockingPark,
}
