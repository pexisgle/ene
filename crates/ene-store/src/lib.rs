//! SQLite-backed implementations of the repository contracts owned by
//! [`ene_presence`], [`ene_companion`], [`ene_permission`],
//! [`ene_credential`], [`ene_inference`], [`ene_learning`], [`ene_task`], and
//! [`ene_action`]; those owners never depend on this crate and program against
//! their own traits. It also implements the preservation-owned local-erasure
//! participants for the owners whose durable master lives here
//! ([`TaskErasureParticipant`], [`ActionErasureParticipant`], and
//! [`InferenceErasureParticipant`]); the Host composition registers them.
//!
//! Concurrency shape: the connection is `Send` but not `Sync`, so an
//! `Arc<std::sync::Mutex<Connection>>` shares it across callers. Each
//! repository method hands its whole critical section — lock, one short
//! [`rusqlite::TransactionBehavior::Immediate`] transaction (or one plain
//! statement for pure loads), drop the guard — to `run_blocking`, so the
//! synchronous `rusqlite` work happens on the blocking pool instead of on an
//! async worker. The guard and any transaction never cross an `.await`: they
//! live and die inside the blocking closure. Values that cross the boundary
//! are bound parameters, never interpolated into SQL text.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use rusqlite::Connection;

mod action;
mod codec;
mod companion;
mod credential;
mod erasure;
mod inference;
mod learning;
mod migrate;
mod permission;
mod presence;
mod preservation;
mod task;
#[cfg(test)]
mod tests;
mod usage_cap;

pub use companion::UndeliveredExcerpt;
pub use erasure::{
    ActionErasureParticipant, CompanionErasureParticipant, ERASURE_SCAN_ROWS,
    InferenceErasureParticipant, LearningErasureParticipant, TaskErasureParticipant,
};

/// Messages carry the short backend cause only. Paths are non-secret but are
/// kept out of messages for operational brevity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("store open failed: {0}")]
    OpenFailed(String),
    #[error("store schema initialization failed: {0}")]
    SchemaFailed(String),
}

/// A panic inside the blocking task is the task's own panic: resume it rather
/// than reporting it as a store failure.
async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(join) => std::panic::resume_unwind(join.into_panic()),
    }
}

/// Coalesced wakeup hint for the presentation subscription (CCT §10.5).
///
/// The store bumps the epoch after any commit that may have inserted an
/// `undelivered` row. The hint carries no state and is never authority:
/// subscribers re-read durable rows after every change, so a duplicated,
/// early, or rolled-back hint is harmless and a lost one only delays
/// delivery until the next hint or connection event. Reads of the
/// `undelivered` table serialize on the store's connection mutex, so a
/// durable query started after a bump always observes the committed row.
#[derive(Clone)]
pub struct UndeliveredSignal {
    epoch: Arc<tokio::sync::watch::Sender<u64>>,
}

impl UndeliveredSignal {
    fn new() -> Self {
        Self {
            epoch: Arc::new(tokio::sync::watch::channel(0).0),
        }
    }

    fn bump(&self) {
        self.epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    /// Subscribes to the coalesced hint. A receiver observes only changes
    /// after it was created; callers that must not lose a registration
    /// subscribe before reading the durable backlog.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.epoch.subscribe()
    }
}

/// SQLite-backed host for every repository contract.
///
/// Cloning is a cheap handle copy over the same connection: the connection
/// table's close admission moves one clone into its `spawn_blocking` section
/// while the owning handle keeps the store.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// Bumped after any commit that may have registered an undelivered row;
    /// see [`UndeliveredSignal`].
    undelivered: UndeliveredSignal,
}

impl Store {
    /// Initializes an empty database or opens the exact current schema.
    /// Unsupported schemas are rejected without changes.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        run_blocking(move || Self::open_sync(&path)).await
    }

    fn open_sync(path: &Path) -> Result<Self, StoreError> {
        let mut conn =
            Connection::open(path).map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::SchemaFailed)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            undelivered: UndeliveredSignal::new(),
        })
    }

    /// Relaxes SQLite durability for test fixtures while keeping the database
    /// file, schema, transaction boundaries, and reopen behavior intact.
    ///
    /// Tests that exercise logical repository/Host behavior do not need an
    /// `fsync` after every short transaction. The explicit opt-in keeps the
    /// production open path unchanged while avoiding that filesystem cost on
    /// Windows CI. The resulting database remains readable by a later normal
    /// [`Store::open`]; this helper only weakens crash/power-loss durability of
    /// the current test connection.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn relax_durability_for_tests(&self) -> Result<(), StoreError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let conn = conn.lock().map_err(|_| {
                StoreError::OpenFailed(String::from("test store connection lock is poisoned"))
            })?;
            conn.pragma_update(None, "journal_mode", "MEMORY")
                .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
            conn.pragma_update(None, "synchronous", "OFF")
                .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
            Ok(())
        })
        .await
    }

    /// In-memory store for this crate's tests only; production opens files.
    #[cfg(test)]
    pub(crate) async fn open_in_memory() -> Result<Self, StoreError> {
        run_blocking(Self::open_in_memory_sync).await
    }

    #[cfg(test)]
    fn open_in_memory_sync() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()
            .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::SchemaFailed)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            undelivered: UndeliveredSignal::new(),
        })
    }

    /// Mechanical exact-text remainder probe over the closed system-wide
    /// canonical content surface the A5 completion boundary verifies, plus
    /// the derived token index and the undelivered references whose canonical
    /// source is gone.
    ///
    /// Test-support only: tests assert `0` after an erasure instead of
    /// re-implementing the column list. The list is the same closed surface
    /// `crate::erasure::system_remainder` uses, so a probe cannot check a
    /// different column set than the completion boundary verifies.
    ///
    /// # Errors
    ///
    /// [`StoreError::OpenFailed`] when the connection cannot be locked.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn count_exact_text_remainder_for_tests(
        &self,
        text: &str,
    ) -> Result<u64, StoreError> {
        let conn = Arc::clone(&self.conn);
        let text = text.to_owned();
        run_blocking(move || {
            let guard = crate::codec::lock_shared(&conn);
            erasure::exact_remainder_probe(&guard, &text)
                .map_err(|error| StoreError::OpenFailed(error.to_string()))
        })
        .await
    }

    /// Subscribes to the coalesced undelivered-registration hint (CCT §10.5).
    ///
    /// Subscribe before reading the durable backlog: a registration that
    /// commits between the read and the wait then changes the epoch, so the
    /// waiter wakes instead of missing the row.
    #[must_use]
    pub fn undelivered_wakeup(&self) -> tokio::sync::watch::Receiver<u64> {
        self.undelivered.subscribe()
    }

    /// Bumps the undelivered hint after `result`, and only when it succeeded.
    ///
    /// Call with the result of a commit that may have inserted an
    /// `undelivered` row. A failed commit rolled back, so there is nothing
    /// new to deliver; a successful one may have, and a spurious bump is
    /// harmless because the hint is never authority.
    fn hint_after_commit<T, E>(&self, result: Result<T, E>) -> Result<T, E> {
        if result.is_ok() {
            self.undelivered.bump();
        }
        result
    }
}
