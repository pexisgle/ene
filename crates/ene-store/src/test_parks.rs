//! Test-only deterministic parks for production mutation boundaries.
//!
//! Each park is first-waiter-only: the first armed caller pauses after
//! signalling entry and before any SQLite lock is taken; later callers pass
//! through. That lets a test hold one in-flight production path while another
//! driver finishes the operation, then release the parked caller to observe
//! a stale no-op. Production builds never compile this module.

use std::sync::atomic::{AtomicBool, Ordering};

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
}
