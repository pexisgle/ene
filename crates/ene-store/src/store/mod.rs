//! Core memory store (`SQLite` + sqlite-vec).
//!
//! Domain queries are split into focused submodules; this file retains the
//! [`MemoryStore`] struct, constructor, and shared low-level helpers.

mod affect;
mod audit;
mod commitment;
mod memory;
mod session;
mod tool;

#[cfg(test)]
mod tests;

use crate::error::EneMemoryError;
use crate::migrator::Migrator;
use chrono::{DateTime, Utc};
pub use ene_core::{
    ActiveSceneSummaryRow, KeyFact, NaturalDecayReport, NewMemorySpan, PendingCandidate,
    PendingCandidateStatus,
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use std::path::Path;

/// A single conversation log entry returned by `get_logs_by_session`.
#[derive(Debug, Clone)]
pub struct ConversationLogEntry {
    /// Speaker role (e.g. "user", "assistant").
    pub role: String,
    /// Message content.
    pub content: String,
    /// When the message was recorded.
    pub created_at: DateTime<Utc>,
}

/// A row of tool embedding data with all fields (domain type from `ene-core`, #302).
pub use ene_core::ToolEmbeddingFieldRow;

/// Registers the sqlite-vec extension globally for the process.
pub fn init_sqlite_vec() {
    use libsqlite3_sys::sqlite3_auto_extension;
    use sqlite_vec::sqlite3_vec_init;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: transmute reinterprets sqlite3_vec_init as the SQLite auto-extension
        // entry-point signature expected by sqlite3_auto_extension.
        let init_fn = unsafe {
            std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut libsqlite3_sys::sqlite3,
                    *mut *mut i8,
                    *const libsqlite3_sys::sqlite3_api_routines,
                ) -> i32,
            >(sqlite3_vec_init as *const ())
        };
        // SAFETY: sqlite3_auto_extension expects a function pointer cast to a void pointer.
        // sqlite3_vec_init is a C function with the correct signature (extern "C" fn()),
        // and transmuting it to *const () is a well-known pattern for registering SQLite
        // extensions. The function pointer remains valid for the lifetime of the process.
        unsafe {
            sqlite3_auto_extension(Some(init_fn));
        }
    });
}

pub(crate) fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len().saturating_mul(4));
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

pub(crate) fn bytes_to_embedding(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

pub(crate) const COSINE_SIMILARITY_SQL: &str = "1.0 - vec_distance_cosine";

pub(crate) const ALLOWED_EMBEDDING_COLS: &[&str] = &["embedding", "memory_embeddings.embedding"];
pub(crate) fn cosine_similarity_expr(
    embedding_col: &str,
    query_bytes: &[u8],
) -> sea_orm::sea_query::Expr {
    use sea_orm::sea_query::Expr;
    assert!(
        ALLOWED_EMBEDDING_COLS.contains(&embedding_col),
        "unexpected embedding column: {embedding_col}"
    );
    let sql = format!("{COSINE_SIMILARITY_SQL}({embedding_col}, ?)");
    Expr::cust_with_values(sql, vec![query_bytes.to_vec()])
}
pub(crate) fn cosine_similarity_filter(
    embedding_col: &str,
    query_bytes: &[u8],
    threshold: f64,
) -> sea_orm::sea_query::Expr {
    use sea_orm::sea_query::Expr;
    assert!(
        ALLOWED_EMBEDDING_COLS.contains(&embedding_col),
        "unexpected embedding column: {embedding_col}"
    );
    let sql = format!("{COSINE_SIMILARITY_SQL}({embedding_col}, ?) >= ?");
    Expr::cust_with_values(
        sql,
        vec![
            sea_orm::Value::from(query_bytes.to_vec()),
            sea_orm::Value::from(threshold),
        ],
    )
}

/// Validates an embedding vector before it is persisted.
///
/// Returns an [`EneMemoryError::InvalidEmbedding`] if the
/// vector's length does not match the store's configured
/// `embedding_dim`, or if it contains any `NaN` or
/// infinite component. Both conditions are fatal for
/// cosine similarity — a single `NaN` poisons the entire
/// `vec_distance_cosine` evaluation at query time, and a
/// length mismatch would silently skew scores against any
/// vector produced by the configured embedder.
pub(crate) fn validate_embedding(
    embedding: &[f32],
    expected_dim: usize,
) -> Result<(), EneMemoryError> {
    if embedding.len() != expected_dim {
        return Err(EneMemoryError::InvalidEmbedding(format!(
            "length {} does not match expected {expected_dim}",
            embedding.len()
        )));
    }
    for (i, &v) in embedding.iter().enumerate() {
        if !v.is_finite() {
            return Err(EneMemoryError::InvalidEmbedding(format!(
                "component {i} is not finite (NaN or Infinity)"
            )));
        }
    }
    Ok(())
}

/// SQLite-backed long-term memory store with vector similarity search.
///
/// Uses `SeaORM` for async database connection management and `sqlite-vec` for cosine-similarity queries.
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
    /// On-disk path when opened from a file (`None` for `:memory:`).
    path: Option<std::path::PathBuf>,
    /// In-memory pending memory candidates awaiting user approval (#174).
    ///
    /// Ephemeral (lost on restart) while the feature is new; a dedicated
    /// `pending_candidates` table will back this in a future migration.
    pending_candidates: parking_lot::Mutex<Vec<PendingCandidate>>,
}

/// Applies the required `SQLite` PRAGMAs to the `sqlx` connect options
/// so that **every** new connection in the pool receives them.
///
/// This is the fix for #419: the previous approach called
/// `execute_unprepared` on the pool handle once after
/// `Database::connect`, which only reached one of the pooled
/// connections. `SqliteConnectOptions::pragma` bakes the PRAGMAs into
/// the connection establishment path, so all eight (or one, for
/// `:memory:`) connections receive the same settings.
///
/// * `journal_mode=WAL` lets readers proceed concurrently with a
///   writer. WAL is a no-op for in-memory databases (`SQLite` returns
///   `memory`), so it is safe to issue unconditionally.
/// * `busy_timeout=5000` (5 seconds) makes concurrent writers wait for
///   the lock instead of failing with `database is locked` immediately.
/// * `foreign_keys=ON` enables enforcement of foreign-key constraints
///   declared in migrations (sqlx already defaults to ON; stated
///   explicitly for clarity).
/// * `synchronous=NORMAL` reduces fsync frequency while remaining safe
///   under WAL.
fn sqlite_connect_pragmas(
    opts: sea_orm::sqlx::sqlite::SqliteConnectOptions,
) -> sea_orm::sqlx::sqlite::SqliteConnectOptions {
    opts.pragma("journal_mode", "WAL")
        .pragma("busy_timeout", "5000")
        .pragma("foreign_keys", "ON")
        .pragma("synchronous", "NORMAL")
}

/// Read applied `SeaORM` migration names from `seaql_migrations`.
async fn applied_migration_names(db: &DatabaseConnection) -> Result<Vec<String>, EneMemoryError> {
    use sea_orm::{DbBackend, Statement};

    // Table may not exist yet on a brand-new database.
    let exists = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type='table' AND name='seaql_migrations'"
                .to_string(),
        ))
        .await?;
    if exists.is_empty() {
        return Ok(Vec::new());
    }
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT version FROM seaql_migrations ORDER BY version".to_string(),
        ))
        .await?;
    let mut names = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get_by_index(0).map_err(|e| {
            EneMemoryError::MemoryStoreConnectionError(format!("decode seaql_migrations: {e}"))
        })?;
        names.push(name);
    }
    Ok(names)
}

impl MemoryStore {
    fn init(
        db: DatabaseConnection,
        embedding_dim: usize,
        path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            db,
            embedding_dim,
            path,
            pending_candidates: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Decode stored embedding bytes.
    pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32> {
        bytes_to_embedding(bytes)
    }

    /// Raw `sea-orm` connection — used by migration helpers.
    pub const fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    /// On-disk path when this store was opened from a file.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Vector dimensionality for this store's embedding model.
    pub const fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    /// Opens a persistent memory store at the given file path with default
    /// [`crate::backup::OpenOptions`].
    ///
    /// Creates the database file if it doesn't exist. Registers the
    /// `sqlite-vec` extension process-globally *before* opening the connection
    /// (required because `sqlite3_auto_extension` only affects connections
    /// opened after the call), then runs database migrations.
    ///
    /// The connection has `journal_mode=WAL`, `busy_timeout=5000`, and
    /// `foreign_keys=ON` set on every pooled connection. WAL lets readers
    /// proceed concurrently with a writer; the busy timeout avoids
    /// spurious `database is locked` errors under contention.
    ///
    /// When pending migrations exist and `backup_on_migrate` is enabled, a
    /// file backup is taken first; on migration failure the backup is
    /// restored (#239).
    pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, EneMemoryError> {
        Self::open_with_options(path, embedding_dim, &crate::backup::OpenOptions::default()).await
    }

    /// Opens a persistent memory store with explicit backup / integrity options (#239).
    pub async fn open_with_options(
        path: &Path,
        embedding_dim: usize,
        options: &crate::backup::OpenOptions,
    ) -> Result<Self, EneMemoryError> {
        let path_str = path.to_str().ok_or_else(|| {
            EneMemoryError::MemoryStoreConnectionError("Invalid path".to_string())
        })?;
        init_sqlite_vec();
        let mut opt = ConnectOptions::new(format!("sqlite:{path_str}?mode=rwc"));
        opt.max_connections(8);
        opt.map_sqlx_sqlite_opts(sqlite_connect_pragmas);
        let db = Database::connect(opt).await?;

        if options.integrity_check_on_open {
            crate::backup::check_integrity(&db).await?;
        }

        let expected: Vec<String> = Migrator::migrations()
            .into_iter()
            .map(|m| m.name().to_string())
            .collect();
        let applied = applied_migration_names(&db).await?;

        let unknown: Vec<&String> = applied
            .iter()
            .filter(|name| !expected.iter().any(|e| e == *name))
            .collect();
        if !unknown.is_empty() {
            return Err(EneMemoryError::SchemaTooNew {
                unknown: unknown
                    .into_iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }

        let pending: Vec<&String> = expected
            .iter()
            .filter(|name| !applied.iter().any(|a| a == *name))
            .collect();

        if pending.is_empty() {
            return Ok(Self::init(db, embedding_dim, Some(path.to_path_buf())));
        }

        let backup_path = if options.backup_on_migrate && path.exists() {
            let meta = std::fs::metadata(path).ok();
            if meta.is_some_and(|m| m.len() > 0) {
                Some(crate::backup::backup_database(path, Some(&db)).await?)
            } else {
                None
            }
        } else {
            None
        };

        match Migrator::up(&db, None).await {
            Ok(()) => {
                if backup_path.is_some() {
                    crate::backup::prune_backups(path, options.max_backups)?;
                }
                Ok(Self::init(db, embedding_dim, Some(path.to_path_buf())))
            }
            Err(cause) => {
                // Drop the pool before restoring so the file is not locked.
                drop(db);
                if let Some(backup) = backup_path {
                    crate::backup::restore_database(&backup, path)?;
                    Err(EneMemoryError::MigrationRolledBack {
                        backup: backup.display().to_string(),
                        cause: cause.to_string(),
                    })
                } else {
                    Err(EneMemoryError::from(cause))
                }
            }
        }
    }

    /// Opens an in-memory memory store (useful for testing).
    ///
    /// Registers the `sqlite-vec` extension process-globally *before* opening
    /// the connection, since `:memory:` reuses a single persistent connection
    /// for the life of the store. Uses `"sqlite::memory:"` as the database
    /// path with a pool limited to one connection. The same PRAGMAs as
    /// [`open`](Self::open) are applied so behavior matches the file-backed
    /// path.
    pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, EneMemoryError> {
        init_sqlite_vec();
        let mut opt = ConnectOptions::new("sqlite::memory:");
        opt.max_connections(1);
        opt.map_sqlx_sqlite_opts(sqlite_connect_pragmas);
        let db = Database::connect(opt).await?;

        Migrator::up(&db, None).await?;

        Ok(Self::init(db, embedding_dim, None))
    }

    /// Run `PRAGMA integrity_check` on the open connection (#239).
    pub async fn check_integrity(&self) -> Result<(), EneMemoryError> {
        crate::backup::check_integrity(&self.db).await
    }

    /// Create a timestamped file backup of this store's database (#239).
    ///
    /// Returns an error when the store is in-memory (no path).
    pub async fn backup(&self) -> Result<std::path::PathBuf, EneMemoryError> {
        let path = self.path().ok_or_else(|| {
            EneMemoryError::BackupError("in-memory store cannot be backed up to a file".into())
        })?;
        crate::backup::backup_database(path, Some(&self.db)).await
    }

    // ── Pending candidate CRUD (#174) ──

    /// Insert a new pending candidate and return its assigned id.
    ///
    /// The candidate starts in [`PendingCandidateStatus::Pending`]. Ids are
    /// assigned monotonically within this store instance.
    pub fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, EneMemoryError> {
        let mut guard = self.pending_candidates.lock();
        let next_id = guard
            .iter()
            .map(|c| c.id)
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let mut stored = candidate;
        stored.id = next_id;
        stored.status = PendingCandidateStatus::Pending;
        guard.push(stored);
        Ok(next_id)
    }

    /// List pending candidates for a character, optionally filtered by status.
    ///
    /// When `status_filter` is `None`, candidates of every status are returned.
    pub fn list_pending_candidates(
        &self,
        character_id: &str,
        status_filter: Option<PendingCandidateStatus>,
    ) -> Result<Vec<PendingCandidate>, EneMemoryError> {
        let guard = self.pending_candidates.lock();
        Ok(guard
            .iter()
            .filter(|c| {
                c.character_id == character_id
                    && status_filter.is_none_or(|status| c.status == status)
            })
            .cloned()
            .collect())
    }

    /// Approve a pending candidate, persisting it to typed memory as active.
    ///
    /// The candidate is inserted via [`Self::insert_typed_memory`] and its
    /// status flipped to [`PendingCandidateStatus::Approved`]. Returns an
    /// error when the candidate is not found or has already been resolved.
    pub async fn approve_pending_candidate(&self, id: i64) -> Result<i64, EneMemoryError> {
        let candidate =
            {
                let guard = self.pending_candidates.lock();
                guard.iter().find(|c| c.id == id).cloned().ok_or_else(|| {
                    EneMemoryError::Other(format!("pending candidate {id} not found"))
                })?
            };
        if candidate.status != PendingCandidateStatus::Pending {
            return Err(EneMemoryError::Other(format!(
                "pending candidate {id} is already {}",
                candidate.status.as_str()
            )));
        }

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: candidate.character_id.clone(),
            user_id: candidate.user_id.clone(),
            kind: candidate.kind,
            title: candidate.title.clone(),
            content: candidate.content.clone(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(candidate.confidence),
            salience: crate::MemorySalience::new(candidate.confidence),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        let memory_id = self.insert_typed_memory(&item).await?;

        let mut guard = self.pending_candidates.lock();
        if let Some(stored) = guard.iter_mut().find(|c| c.id == id) {
            stored.status = PendingCandidateStatus::Approved;
        }
        Ok(memory_id)
    }

    /// Resolve (approve or reject) a pending candidate by id.
    ///
    /// When `approved` is `true`, the candidate is persisted to typed memory
    /// (see [`Self::approve_pending_candidate`]); when `false`, its status is
    /// set to [`PendingCandidateStatus::Rejected`].
    pub async fn resolve_pending_candidate(
        &self,
        id: i64,
        approved: bool,
    ) -> Result<(), EneMemoryError> {
        if approved {
            self.approve_pending_candidate(id).await?;
            return Ok(());
        }
        let mut guard = self.pending_candidates.lock();
        let candidate = guard
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| EneMemoryError::Other(format!("pending candidate {id} not found")))?;
        if candidate.status != PendingCandidateStatus::Pending {
            return Err(EneMemoryError::Other(format!(
                "pending candidate {id} is already {}",
                candidate.status.as_str()
            )));
        }
        candidate.status = PendingCandidateStatus::Rejected;
        Ok(())
    }
}
