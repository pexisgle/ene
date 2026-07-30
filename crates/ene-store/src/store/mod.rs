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

    // ── Pending candidate CRUD (#174, #420) ──

    /// Insert a new pending candidate and return its assigned id.
    ///
    /// The candidate is persisted to the `pending_candidates` table (#420) so
    /// it survives restarts and is shared across every [`MemoryStore`]
    /// instance opened on the same database. The stored status is forced to
    /// [`PendingCandidateStatus::Pending`] regardless of the value supplied;
    /// the id comes from the table's `AUTOINCREMENT` primary key.
    pub async fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, EneMemoryError> {
        use crate::entities::pending_candidates::ActiveModel;
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let active = ActiveModel {
            id: sea_orm::NotSet,
            character_id: Set(candidate.character_id),
            user_id: Set(candidate.user_id),
            title: Set(candidate.title),
            content: Set(candidate.content),
            kind: Set(candidate.kind.as_str().to_string()),
            confidence: Set(candidate.confidence),
            reason_detail: Set(candidate.reason_detail),
            source_quote: Set(candidate.source_quote),
            existing_memory_id: Set(candidate.existing_memory_id),
            status: Set(PendingCandidateStatus::Pending.as_str().to_string()),
            created_at: Set(candidate.created_at),
        };
        let inserted = active.insert(&self.db).await?;
        Ok(inserted.id)
    }

    /// List pending candidates for a character, optionally filtered by status.
    ///
    /// When `status_filter` is `None`, candidates of every status are
    /// returned, oldest first. Rows whose stored status label is unrecognized
    /// are excluded (fail closed) rather than surfaced as `Pending` (#420
    /// review).
    pub async fn list_pending_candidates(
        &self,
        character_id: &str,
        status_filter: Option<PendingCandidateStatus>,
    ) -> Result<Vec<PendingCandidate>, EneMemoryError> {
        use crate::entities::pending_candidates::{Column, Entity, model_to_dto};
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

        let mut query = Entity::find().filter(Column::CharacterId.eq(character_id));
        if let Some(status) = status_filter {
            query = query.filter(Column::Status.eq(status.as_str()));
        }
        let rows = query
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(&self.db)
            .await?;
        Ok(rows.into_iter().filter_map(model_to_dto).collect())
    }

    /// Approve a pending candidate, persisting it to typed memory as active.
    ///
    /// Race-safe (#420 review): the stored status is flipped to
    /// [`PendingCandidateStatus::Approved`] with a *conditional* update
    /// (`WHERE id = ? AND status = 'pending'`) **before** the typed memory is
    /// inserted, and the insert only proceeds if this call won the flip.
    /// Because the queue is shared across every [`MemoryStore`] instance on
    /// the same database, two concurrent approvals can no longer both pass a
    /// read-check and both insert — exactly one `UPDATE` matches, the loser
    /// sees `rows_affected == 0` and errors without inserting a duplicate
    /// typed memory. Returns an error when the candidate is not found or has
    /// already been resolved.
    pub async fn approve_pending_candidate(&self, id: i64) -> Result<i64, EneMemoryError> {
        // Claim the row first; only the winner proceeds to insert.
        let claimed = self
            .claim_pending_candidate(id, PendingCandidateStatus::Approved)
            .await?;
        if !claimed {
            return Err(EneMemoryError::Other(format!(
                "pending candidate {id} not found or already resolved"
            )));
        }

        // Re-read the now-approved row to build the typed memory. The status
        // flip already serialized concurrent resolvers, so this read is stable.
        let candidate = self.load_pending_candidate(id).await?;

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
            supersedes_id: candidate.existing_memory_id,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        self.insert_typed_memory(&item).await
    }

    /// Resolve (approve or reject) a pending candidate by id.
    ///
    /// When `approved` is `true`, the candidate is persisted to typed memory
    /// (see [`Self::approve_pending_candidate`]); when `false`, its status is
    /// set to [`PendingCandidateStatus::Rejected`]. Both paths claim the row
    /// with a conditional status flip first, so concurrent resolvers cannot
    /// double-apply (#420 review).
    pub async fn resolve_pending_candidate(
        &self,
        id: i64,
        approved: bool,
    ) -> Result<(), EneMemoryError> {
        if approved {
            self.approve_pending_candidate(id).await?;
            return Ok(());
        }
        let claimed = self
            .claim_pending_candidate(id, PendingCandidateStatus::Rejected)
            .await?;
        if !claimed {
            return Err(EneMemoryError::Other(format!(
                "pending candidate {id} not found or already resolved"
            )));
        }
        Ok(())
    }

    /// Enforce the pending-candidate retention policy for a character (#420).
    ///
    /// Two independent limits, either of which may be disabled with `0`:
    ///
    /// * **Age** (`max_age_days > 0`): delete candidates of *any* status whose
    ///   `created_at` is older than `now - max_age_days`. This deliberately
    ///   includes **unanswered** (`pending`) candidates: a candidate left
    ///   unreviewed past the age limit expires so the queue cannot grow
    ///   without bound even when the count cap is disabled. Resolved
    ///   candidates (approved / rejected) have no further UI value once stale,
    ///   so they are swept too.
    /// * **Count** (`max_per_character > 0`): cap the live `pending` queue; when
    ///   it exceeds the cap, delete the oldest overflow rows.
    ///
    /// Both passes are scoped to `character_id` and, when `user_id` is
    /// `Some`, to that user's rows plus character-shared rows (`user_id = ""`)
    /// — mirroring the visibility rule used by natural decay — so on a
    /// multi-user database one user's candidates cannot evict another's under
    /// the count cap. Returns the total number of rows removed. Idempotent and
    /// cheap (pure SQL `DELETE`), so it is safe to run on every post-turn
    /// forgetting pass.
    pub async fn prune_pending_candidates(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        max_age_days: u32,
        max_per_character: usize,
        now: DateTime<Utc>,
    ) -> Result<usize, EneMemoryError> {
        use crate::entities::pending_candidates::{Column, Entity};
        use sea_orm::{
            ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
        };

        // Visibility condition shared by both passes: the character plus, when
        // a user is given, that user's rows and character-shared rows.
        let scope = pending_candidate_scope(character_id, user_id);

        let mut removed = 0usize;

        if max_age_days > 0 {
            let cutoff = now
                .checked_sub_signed(chrono::Duration::days(i64::from(max_age_days)))
                .unwrap_or(now);
            let aged = Entity::delete_many()
                .filter(scope.clone())
                .filter(Column::CreatedAt.lt(cutoff))
                .exec(&self.db)
                .await?;
            removed = removed.saturating_add(aged.rows_affected as usize);
        }

        if max_per_character > 0 {
            // Count the live queue, then delete the oldest overflow beyond the
            // cap. Doing it as a count + targeted delete (rather than a
            // `NOT IN (SELECT ... LIMIT)` subquery) keeps the SQL portable and
            // easy to reason about.
            let pending_count = Entity::find()
                .filter(scope.clone())
                .filter(Column::Status.eq(PendingCandidateStatus::Pending.as_str()))
                .count(&self.db)
                .await? as usize;

            if pending_count > max_per_character {
                let overflow = pending_count - max_per_character;
                // Ids of the oldest `overflow` pending rows.
                let drop_ids: Vec<i64> = Entity::find()
                    .filter(scope)
                    .filter(Column::Status.eq(PendingCandidateStatus::Pending.as_str()))
                    .order_by_asc(Column::CreatedAt)
                    .order_by_asc(Column::Id)
                    .limit(overflow as u64)
                    .all(&self.db)
                    .await?
                    .into_iter()
                    .map(|m| m.id)
                    .collect();

                if !drop_ids.is_empty() {
                    let culled = Entity::delete_many()
                        .filter(Column::Id.is_in(drop_ids))
                        .exec(&self.db)
                        .await?;
                    removed = removed.saturating_add(culled.rows_affected as usize);
                }
            }
        }

        if removed > 0 {
            tracing::info!(
                component = "MemoryStore",
                character_id,
                user_id = user_id.unwrap_or(""),
                removed,
                max_age_days,
                max_per_character,
                "Pruned pending memory candidates via retention policy"
            );
        }

        Ok(removed)
    }

    /// Load a single pending candidate row by id (#420).
    ///
    /// Errors when the row is missing or carries an unrecognized status label
    /// (fail closed, matching [`Self::list_pending_candidates`]).
    async fn load_pending_candidate(&self, id: i64) -> Result<PendingCandidate, EneMemoryError> {
        use crate::entities::pending_candidates::{Entity, model_to_dto};
        use sea_orm::EntityTrait;

        let model = Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or_else(|| EneMemoryError::Other(format!("pending candidate {id} not found")))?;
        model_to_dto(model).ok_or_else(|| {
            EneMemoryError::Other(format!("pending candidate {id} has an unrecognized status"))
        })
    }

    /// Atomically claim a pending candidate by flipping its status from
    /// [`PendingCandidateStatus::Pending`] to `target` (#420 review).
    ///
    /// Implemented as a single conditional `UPDATE ... WHERE id = ? AND status
    /// = 'pending'`, so it doubles as a compare-and-swap: returns `true` only
    /// when this call actually flipped a row. A concurrent resolver (or a
    /// retry after a crash) loses the race and sees `false`, which callers
    /// turn into an error instead of double-applying. Returns `false` when the
    /// row is missing or already resolved.
    async fn claim_pending_candidate(
        &self,
        id: i64,
        target: PendingCandidateStatus,
    ) -> Result<bool, EneMemoryError> {
        use crate::entities::pending_candidates::{Column, Entity};
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

        let res = Entity::update_many()
            .col_expr(
                Column::Status,
                sea_orm::sea_query::Expr::value(target.as_str().to_string()),
            )
            .filter(Column::Id.eq(id))
            .filter(Column::Status.eq(PendingCandidateStatus::Pending.as_str()))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    /// Backdate a pending candidate's `created_at` for retention tests (#420).
    #[doc(hidden)]
    pub async fn test_backdate_pending_candidate(
        &self,
        id: i64,
        days_ago: i64,
    ) -> Result<bool, EneMemoryError> {
        use crate::entities::pending_candidates::{ActiveModel, Entity};
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let Some(model) = Entity::find_by_id(id).one(&self.db).await? else {
            return Ok(false);
        };
        let anchor = Utc::now()
            .checked_sub_signed(chrono::Duration::days(days_ago))
            .unwrap_or_else(Utc::now);
        let mut active: ActiveModel = model.into();
        active.created_at = Set(anchor);
        active.update(&self.db).await?;
        Ok(true)
    }
}

/// Visibility condition for pending-candidate retention queries (#420 review).
///
/// Scopes to `character_id` and, when `user_id` is `Some`, to that user's rows
/// plus character-shared rows (`user_id = ""`) — mirroring
/// `store::memory::user_visibility_condition` used by natural decay so one
/// user's candidates cannot evict another's on a multi-user database.
fn pending_candidate_scope(character_id: &str, user_id: Option<&str>) -> sea_orm::Condition {
    use crate::entities::pending_candidates::Column;
    use sea_orm::ColumnTrait;

    let mut condition = sea_orm::Condition::all().add(Column::CharacterId.eq(character_id));
    if let Some(uid) = user_id {
        condition = condition.add(
            sea_orm::Condition::any()
                .add(Column::UserId.eq(uid))
                .add(Column::UserId.eq("")),
        );
    }
    condition
}
