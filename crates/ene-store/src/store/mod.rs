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

/// A row of tool embedding data with all fields (domain type from `ene-core`).
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

/// Creates the `vec0` ANN index tables and sync triggers if they do not
/// already exist.
///
/// Called during store initialization after migrations. For databases
/// upgraded from a pre-vec0 schema the migration
/// `m20260731_000006_vec0_embedding_index` already created and backfilled
/// the tables; this function is a no-op in that case (`IF NOT EXISTS`).
/// For fresh databases (no embeddings at migration time) this is where
/// the tables are first created.
///
/// If the embedding dimension changes (e.g. user switches embedding models),
/// the vec0 tables are dropped and recreated empty at the new dimension.
/// Existing vectors of the old dimension cannot be inserted into the
/// recreated `float[N]` table, so they are left out and a re-embedding pass
/// is required (a warning is logged). KNN search returns no rows for the
/// stale vectors until then; the store itself stays usable.
///
/// Sync triggers on `memory_embeddings` and `tool_embedding_index` keep
/// the `vec0` shadow tables consistent through all write paths, including
/// `ON DELETE CASCADE` from `typed_memories`.
pub(crate) async fn ensure_vec0_index(
    db: &DatabaseConnection,
    embedding_dim: usize,
) -> Result<(), EneMemoryError> {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    let dim = embedding_dim;

    // Check if vec_memory_embeddings exists and has the correct dimension.
    // The vec0 column definition bakes in `float[N]` at creation time, so a
    // dimension change (e.g. switching embedding models) requires a rebuild.
    // We read the stored CREATE statement from sqlite_master and check for
    // the expected `float[{dim}]` substring. Both shadow tables are checked:
    // a dimension change rebuilds both, and only re-creating one of them
    // would leave the pair inconsistent.
    let needs_recreate = {
        let check = |table: &str| {
            db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
                [table.into()],
            ))
        };
        let mem_sql = match check("vec_memory_embeddings").await? {
            Some(row) => Some(row.try_get_by_index::<String>(0).map_err(|e| {
                EneMemoryError::MemoryStoreConnectionError(format!("read sqlite_master: {e}"))
            })?),
            None => None,
        };
        let tool_sql = match check("vec_tool_embeddings").await? {
            Some(row) => Some(row.try_get_by_index::<String>(0).map_err(|e| {
                EneMemoryError::MemoryStoreConnectionError(format!("read sqlite_master: {e}"))
            })?),
            None => None,
        };
        match (mem_sql, tool_sql) {
            (Some(m), Some(t)) => {
                !m.contains(&format!("float[{dim}]")) || !t.contains(&format!("float[{dim}]"))
            }
            // One or both tables missing: recreate path below creates them.
            _ => true,
        }
    };

    // Whether the shadow tables existed at all before this call. A dimension
    // change drops them, but the old-dimension vectors cannot be inserted
    // into the recreated `float[N]` table, so the backfill below only runs
    // when the tables are being created for the *first* time (fresh DB or
    // upgrade path with no prior vec0 tables).
    let existed_before = needs_recreate
        && db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                ["vec_memory_embeddings".into()],
            ))
            .await?
            .is_some();

    if needs_recreate {
        db.execute_unprepared("DROP TABLE IF EXISTS vec_memory_embeddings")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS vec_tool_embeddings")
            .await?;
    }

    // character_id is denormalized from typed_memories so the KNN query
    // can scope to a single character via a vec0 metadata filter.
    db.execute_unprepared(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_memory_embeddings USING vec0( \
             memory_embedding_id integer primary key, \
             embedding float[{dim}] distance_metric=cosine, \
             character_id text, \
             model_name text, \
             field text \
         )"
    ))
    .await?;

    db.execute_unprepared(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_tool_embeddings USING vec0( \
             tool_embedding_id integer primary key, \
             embedding float[{dim}] distance_metric=cosine, \
             tool_name text \
         )"
    ))
    .await?;

    // Backfill the freshly-created tables. This only happens when the
    // shadow tables did not exist before (first creation): after a
    // dimension change the old-dimension vectors are incompatible with the
    // new `float[N]` definition and are left out, requiring a re-embedding
    // pass (logged below).
    if needs_recreate && !existed_before {
        db.execute_unprepared(
            "INSERT INTO vec_memory_embeddings(memory_embedding_id, embedding, character_id, model_name, field) \
             SELECT me.id, me.embedding, tm.character_id, me.model_name, me.field \
             FROM memory_embeddings me \
             INNER JOIN typed_memories tm ON tm.id = me.memory_item_id",
        )
        .await?;

        db.execute_unprepared(
            "INSERT INTO vec_tool_embeddings(tool_embedding_id, embedding, tool_name) \
             SELECT id, embedding, tool_name FROM tool_embedding_index",
        )
        .await?;
    } else if needs_recreate {
        // Dimension change: the old vectors cannot be re-inserted into the
        // recreated tables. The store remains usable (KNN simply returns no
        // rows until re-embedding), which is strictly better than failing
        // startup forever on a model switch.
        tracing::warn!(
            component = "ene-store",
            new_dimension = dim,
            "Embedding dimension changed; vec0 index recreated empty, \
             re-embedding of existing memories/tools required"
        );
    }

    // vec0 returns SQLITE_ERROR (not SQLITE_CONSTRAINT) on duplicate PKs, so
    // INSERT OR REPLACE does not work. The update trigger uses DELETE + INSERT
    // instead. character_id is resolved from typed_memories via a correlated
    // SELECT in the trigger body.
    db.execute_unprepared(
        "CREATE TRIGGER IF NOT EXISTS trg_vec_mem_ai AFTER INSERT ON memory_embeddings BEGIN \
             INSERT INTO vec_memory_embeddings(memory_embedding_id, embedding, character_id, model_name, field) \
             SELECT new.id, new.embedding, tm.character_id, new.model_name, new.field \
             FROM typed_memories tm WHERE tm.id = new.memory_item_id; \
         END",
    )
    .await?;

    db.execute_unprepared(
        "CREATE TRIGGER IF NOT EXISTS trg_vec_mem_au AFTER UPDATE ON memory_embeddings BEGIN \
             DELETE FROM vec_memory_embeddings WHERE memory_embedding_id = old.id; \
             INSERT INTO vec_memory_embeddings(memory_embedding_id, embedding, character_id, model_name, field) \
             SELECT new.id, new.embedding, tm.character_id, new.model_name, new.field \
             FROM typed_memories tm WHERE tm.id = new.memory_item_id; \
         END",
    )
    .await?;

    db.execute_unprepared(
        "CREATE TRIGGER IF NOT EXISTS trg_vec_mem_ad AFTER DELETE ON memory_embeddings BEGIN \
             DELETE FROM vec_memory_embeddings WHERE memory_embedding_id = old.id; \
         END",
    )
    .await?;

    db.execute_unprepared(
        "CREATE TRIGGER IF NOT EXISTS trg_vec_tool_ai AFTER INSERT ON tool_embedding_index BEGIN \
             INSERT INTO vec_tool_embeddings(tool_embedding_id, embedding, tool_name) \
             VALUES (new.id, new.embedding, new.tool_name); \
         END",
    )
    .await?;

    db.execute_unprepared(
        "CREATE TRIGGER IF NOT EXISTS trg_vec_tool_au AFTER UPDATE ON tool_embedding_index BEGIN \
             DELETE FROM vec_tool_embeddings WHERE tool_embedding_id = old.id; \
             INSERT INTO vec_tool_embeddings(tool_embedding_id, embedding, tool_name) \
             VALUES (new.id, new.embedding, new.tool_name); \
         END",
    )
    .await?;

    db.execute_unprepared(
        "CREATE TRIGGER IF NOT EXISTS trg_vec_tool_ad AFTER DELETE ON tool_embedding_index BEGIN \
             DELETE FROM vec_tool_embeddings WHERE tool_embedding_id = old.id; \
         END",
    )
    .await?;

    Ok(())
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

#[cfg(test)]
pub(crate) const COSINE_SIMILARITY_SQL: &str = "1.0 - vec_distance_cosine";

/// An embedding column that cosine-similarity SQL may reference.
///
/// A `&str` guarded by an allowlist and an `assert!` would leave a panic path
/// in the code (`clippy::panic` does not catch `assert!`) and rely on a
/// runtime check to uphold the "only these columns" invariant. Modelling the
/// two permitted columns as an enum makes any other value unrepresentable: the
/// SQL fragment is derived from the variant, so there is nothing to validate
/// and no way to reach an unexpected column.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingCol {
    /// The unqualified `embedding` column of the table being queried.
    Bare,
    /// The `memory_embeddings.embedding` column, qualified for joins where the
    /// bare name would be ambiguous.
    Qualified,
}

#[cfg(test)]
impl EmbeddingCol {
    /// The SQL column reference interpolated into the cosine-similarity
    /// expression.
    const fn as_sql(self) -> &'static str {
        match self {
            Self::Bare => "embedding",
            Self::Qualified => "memory_embeddings.embedding",
        }
    }
}

#[cfg(test)]
pub(crate) fn cosine_similarity_expr(
    embedding_col: EmbeddingCol,
    query_bytes: &[u8],
) -> sea_orm::sea_query::Expr {
    use sea_orm::sea_query::Expr;
    let sql = format!("{COSINE_SIMILARITY_SQL}({}, ?)", embedding_col.as_sql());
    Expr::cust_with_values(sql, vec![query_bytes.to_vec()])
}
#[cfg(test)]
pub(crate) fn cosine_similarity_filter(
    embedding_col: EmbeddingCol,
    query_bytes: &[u8],
    threshold: f64,
) -> sea_orm::sea_query::Expr {
    use sea_orm::sea_query::Expr;
    let sql = format!(
        "{COSINE_SIMILARITY_SQL}({}, ?) >= ?",
        embedding_col.as_sql()
    );
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
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
    /// On-disk path when opened from a file (`None` for `:memory:`).
    path: Option<std::path::PathBuf>,
}

/// Applies the required `SQLite` PRAGMAs to the `sqlx` connect options
/// so that **every** new connection in the pool receives them.
///
/// `PRAGMA` settings are per-connection: executing them once on the pool
/// handle after `Database::connect` only reaches whichever connection ran
/// the statement, leaving the rest with defaults. `SqliteConnectOptions::pragma`
/// bakes the PRAGMAs into the connection establishment path, so all pooled
/// connections receive the same settings.
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
    /// restored.
    pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, EneMemoryError> {
        Self::open_with_options(path, embedding_dim, &crate::backup::OpenOptions::default()).await
    }

    /// Opens a persistent memory store with explicit backup / integrity options.
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
        // SQLite `:memory:` databases are per-connection: a pool wider than
        // one would hand later requests a fresh empty DB instead of the
        // migrated one (see `open_in_memory`).
        opt.max_connections(if path_str == ":memory:" { 1 } else { 8 });
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
            ensure_vec0_index(&db, embedding_dim).await?;
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
                ensure_vec0_index(&db, embedding_dim).await?;
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

        ensure_vec0_index(&db, embedding_dim).await?;

        Ok(Self::init(db, embedding_dim, None))
    }

    /// Run `PRAGMA integrity_check` on the open connection.
    pub async fn check_integrity(&self) -> Result<(), EneMemoryError> {
        crate::backup::check_integrity(&self.db).await
    }

    /// Create a timestamped file backup of this store's database.
    ///
    /// Returns an error when the store is in-memory (no path).
    pub async fn backup(&self) -> Result<std::path::PathBuf, EneMemoryError> {
        let path = self.path().ok_or_else(|| {
            EneMemoryError::BackupError("in-memory store cannot be backed up to a file".into())
        })?;
        crate::backup::backup_database(path, Some(&self.db)).await
    }

    /// Insert a new pending candidate and return its assigned id.
    ///
    /// The candidate is persisted to the `pending_candidates` table so
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
    /// are excluded (fail closed) rather than surfaced as `Pending`.
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
    /// Race-safe: the stored status is flipped to
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
    /// double-apply.
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

    /// Enforce the pending-candidate retention policy for a character.
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

    /// Load a single pending candidate row by id.
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
    /// [`PendingCandidateStatus::Pending`] to `target`.
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

    /// Backdate a pending candidate's `created_at` for retention tests.
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

/// Visibility condition for pending-candidate retention queries.
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
