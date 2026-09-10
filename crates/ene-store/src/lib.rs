//! SQLite-backed durable repositories for the Stage 2 owners.
//!
//! [`Store`] owns one `rusqlite::Connection` and implements every repository
//! contract owned elsewhere: [`ene_presence::PresenceRepository`],
//! [`ene_companion::CompanionRepository`], [`ene_companion::HistoryRepository`],
//! [`ene_companion::UndeliveredRepository`],
//! [`ene_permission::ConsentRepository`],
//! [`ene_credential::CredentialRefRepository`],
//! [`ene_credential::CredentialApprovalRepository`],
//! [`ene_credential::DevicePairingRepository`], and
//! [`ene_inference::UsageRepository`]. Owners never depend on this crate; they
//! program against their own traits.
//!
//! Layout: this file holds [`Store`] and [`StoreError`] plus the blocking
//! `run_blocking` bridge; `codec` holds the row codecs and shared SQL
//! fragments; `migrate` holds forward-only schema setup; `presence`,
//! `companion`, `permission`, `credential`, and `inference` each hold the
//! repository implementations for one owner group; `tests` holds the store
//! behavior tests.
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

/// Failures opening or migrating the SQLite backing file.
///
/// Messages carry the short backend cause only. Paths are non-secret but are
/// kept out of messages for operational brevity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// Opening the database file failed.
    #[error("store open failed: {0}")]
    OpenFailed(String),
    /// Schema migration failed.
    #[error("store migration failed: {0}")]
    MigrationFailed(String),
}

/// Runs one synchronous SQLite critical section on the blocking pool.
///
/// A panic inside the blocking task is the task's own panic: resume it
/// rather than reporting it as a store failure.
async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(join) => std::panic::resume_unwind(join.into_panic()),
    }
}

/// SQLite-backed host for every Stage 2 repository contract.
///
/// The single `rusqlite::Connection` is `Send` but not `Sync`; an
/// `Arc<std::sync::Mutex<Connection>>` shares it across the repository
/// implementations. Each method runs its whole critical section on the
/// blocking pool through `run_blocking`: it locks, runs one
/// [`rusqlite::TransactionBehavior::Immediate`] transaction (or one plain statement for
/// pure loads), drops the guard, and returns, so the guard and any transaction
/// live entirely inside the blocking closure and never cross an `.await`.
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (or creates) the file-backed store and runs migrations.
    ///
    /// Reopening an existing file is idempotent: the schema setup and the
    /// running-companion seed tolerate an already-migrated database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OpenFailed`] when the file cannot be opened and
    /// [`StoreError::MigrationFailed`] when the schema cannot be prepared.
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

    /// Opens an in-memory store and runs migrations, for tests.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OpenFailed`] when the database cannot be created
    /// and [`StoreError::MigrationFailed`] when the schema cannot be prepared.
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
