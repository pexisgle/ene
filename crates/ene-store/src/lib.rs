//! SQLite-backed implementations of the repository contracts owned by
//! [`ene_presence`], [`ene_companion`], [`ene_permission`],
//! [`ene_credential`], and [`ene_inference`]; those owners never depend on
//! this crate and program against their own traits.
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

mod codec;
mod companion;
mod credential;
mod inference;
mod migrate;
mod permission;
mod presence;
#[cfg(test)]
mod tests;

/// Messages carry the short backend cause only. Paths are non-secret but are
/// kept out of messages for operational brevity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("store open failed: {0}")]
    OpenFailed(String),
    #[error("store migration failed: {0}")]
    MigrationFailed(String),
}

/// A panic inside the blocking task is the task's own panic: resume it
/// rather than reporting it as a store failure.
async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(join) => std::panic::resume_unwind(join.into_panic()),
    }
}

/// SQLite-backed host for every repository contract.
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Reopening an existing file is idempotent: the schema setup and the
    /// running-companion seed tolerate an already-migrated database.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        run_blocking(move || Self::open_sync(&path)).await
    }

    fn open_sync(path: &Path) -> Result<Self, StoreError> {
        let mut conn =
            Connection::open(path).map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::MigrationFailed)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn open_in_memory() -> Result<Self, StoreError> {
        run_blocking(Self::open_in_memory_sync).await
    }

    fn open_in_memory_sync() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()
            .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::MigrationFailed)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}
