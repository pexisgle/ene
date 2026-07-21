use crate::entities;
use crate::error::MemoryError;
use crate::migrator::Migrator;
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};

use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm_migration::MigratorTrait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Input for inserting a compressed conversation span (#79 / #98).
#[derive(Debug, Clone)]
pub struct NewMemorySpan {
    /// Session this span belongs to.
    pub session_id: String,
    /// First turn index in the span.
    pub turn_start: i32,
    /// Last turn index in the span.
    pub turn_end: i32,
    /// Raw excerpt from source logs.
    pub raw_excerpt: Option<String>,
    /// Compressed summary (empty until compression runs).
    pub compressed_summary: Option<String>,
    /// Compression level (0 = scene, 1 = chapter, 2 = arc).
    pub compression_level: i32,
}

/// Active scene summary row for prompt injection (#79).
#[derive(Debug, Clone)]
pub struct ActiveSceneSummaryRow {
    /// Span database id.
    pub span_id: i64,
    /// Summary text.
    pub summary: String,
    /// Compression level.
    pub compression_level: i32,
}

/// A row of tool embedding data: `(tool_name, field, field_key, version_hash, model_name, embedding_vec, source_text)`.
pub type ToolEmbeddingFieldRow = (String, String, String, String, String, Vec<f32>, String);

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

fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len().saturating_mul(4));
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

fn bytes_to_embedding(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn audit_row_to_entry(row: entities::audit_log::Model) -> crate::audit::AuditEntry {
    crate::audit::AuditEntry {
        id: row.id,
        turn_id: row.turn_id,
        tool_name: row.tool_name,
        action: row.action,
        target: row.target,
        decision: crate::audit::AuditDecision::parse(&row.decision),
        success: row.success != 0,
        redacted_args: row.redacted_args,
        created_at: row.created_at,
    }
}

fn log_row_to_message(row: entities::conversation_logs::Model) -> crate::export::ExportedMessage {
    crate::export::ExportedMessage {
        role: row.role,
        content: row.content,
        created_at: row.created_at,
    }
}

const COSINE_SIMILARITY_SQL: &str = "1.0 - vec_distance_cosine";

const ALLOWED_EMBEDDING_COLS: &[&str] = &["embedding", "memory_embeddings.embedding"];

fn cosine_similarity_expr(embedding_col: &str, query_bytes: &[u8]) -> sea_orm::sea_query::Expr {
    use sea_orm::sea_query::Expr;
    assert!(
        ALLOWED_EMBEDDING_COLS.contains(&embedding_col),
        "unexpected embedding column: {embedding_col}"
    );
    let sql = format!("{COSINE_SIMILARITY_SQL}({embedding_col}, ?)");
    Expr::cust_with_values(sql, vec![query_bytes.to_vec()])
}

fn cosine_similarity_filter(
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
/// Returns an [`MemoryError::InvalidEmbedding`] if the
/// vector's length does not match the store's configured
/// `embedding_dim`, or if it contains any `NaN` or
/// infinite component. Both conditions are fatal for
/// cosine similarity — a single `NaN` poisons the entire
/// `vec_distance_cosine` evaluation at query time, and a
/// length mismatch would silently skew scores against any
/// vector produced by the configured embedder.
fn validate_embedding(embedding: &[f32], expected_dim: usize) -> Result<(), MemoryError> {
    if embedding.len() != expected_dim {
        return Err(MemoryError::InvalidEmbedding(format!(
            "length {} does not match expected {expected_dim}",
            embedding.len()
        )));
    }
    for (i, &v) in embedding.iter().enumerate() {
        if !v.is_finite() {
            return Err(MemoryError::InvalidEmbedding(format!(
                "component {i} is not finite (NaN or Infinity)"
            )));
        }
    }
    Ok(())
}

/// Memory statuses eligible for hybrid recall: `active`, `faded`, and `disputed`.
///
/// Archived / superseded / user-deleted memories are excluded from recall
/// unless a caller explicitly opts in (see `list_typed_memories`).
const RECALLABLE_STATUSES: [&str; 3] = [
    crate::MemoryStatus::Active.as_str(),
    crate::MemoryStatus::Faded.as_str(),
    crate::MemoryStatus::Disputed.as_str(),
];

/// Builds the user-visibility filter for typed-memory queries.
///
/// A memory is visible to `user_id` when it is owned by that user **or**
/// has no owner (empty `user_id`), so shared / character-level memories
/// remain recallable across users.
fn user_visibility_condition(user_id: &str) -> sea_orm::Condition {
    sea_orm::Condition::any()
        .add(entities::typed_memories::Column::UserId.eq(user_id))
        .add(entities::typed_memories::Column::UserId.eq(""))
}

/// A key-value fact about the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFact {
    /// The key identifier for this fact.
    pub key: String,
    /// The value associated with the key.
    pub value: String,
}

/// SQLite-backed long-term memory store with vector similarity search.
///
/// Uses `SeaORM` for async database connection management and `sqlite-vec` for cosine-similarity queries.
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
}

/// Result of a natural-decay batch run (#76).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NaturalDecayReport {
    /// Memories transitioned to `faded`.
    pub faded_count: usize,
    /// Memories transitioned to `archived`.
    pub archived_count: usize,
}

/// Convert a typed memory model row to a [`crate::MemoryItem`].
#[expect(
    clippy::unnecessary_wraps,
    reason = "store helper signature returns Result for uniform error propagation"
)]
fn model_to_memory_item(
    m: entities::typed_memories::Model,
) -> Result<crate::MemoryItem, MemoryError> {
    Ok(crate::MemoryItem {
        id: Some(m.id),
        scope: str_to_scope(&m.scope),
        character_id: m.character_id,
        user_id: m.user_id,
        kind: str_to_kind(&m.kind),
        title: m.title,
        content: m.content,
        source: str_to_source(&m.source),
        source_ref: m.source_ref,
        confidence: crate::MemoryConfidence::new(m.confidence),
        salience: crate::MemorySalience::new(m.salience),
        affect: crate::AffectAnnotation {
            valence: m.affective_valence,
            arousal: m.affective_arousal,
        },
        relationship_impact: m.relationship_impact,
        access_count: m.access_count,
        last_accessed_at: m.last_accessed_at,
        created_at: m.created_at,
        updated_at: m.updated_at,
        valid_from: m.valid_from,
        valid_until: m.valid_until,
        status: str_to_status(&m.status),
        supersedes_id: m.supersedes_id,
        pinned: m.pinned != 0,
        faded_at: m.faded_at,
        commitment_id: m.commitment_id,
    })
}

/// Convert a commitment model row to a [`crate::Commitment`].
#[expect(
    clippy::unnecessary_wraps,
    reason = "store helper signature returns Result for uniform error propagation"
)]
fn model_to_commitment(m: entities::commitments::Model) -> Result<crate::Commitment, MemoryError> {
    Ok(crate::Commitment {
        id: Some(m.id),
        character_id: m.character_id,
        user_id: m.user_id,
        title: m.title,
        description: m.description,
        status: crate::CommitmentStatus::from_db_str(&m.status),
        due_at: m.due_at,
        due_label: m.due_label,
        created_at: m.created_at,
        updated_at: m.updated_at,
        completed_at: m.completed_at,
    })
}

fn str_to_kind(s: &str) -> crate::MemoryKind {
    crate::MemoryKind::from_db_str(s)
}

fn str_to_scope(s: &str) -> crate::MemoryScope {
    crate::MemoryScope::from_db_str(s)
}

fn str_to_source(s: &str) -> crate::MemorySource {
    crate::MemorySource::from_db_str(s)
}

fn str_to_status(s: &str) -> crate::MemoryStatus {
    crate::MemoryStatus::from_db_str(s)
}

const fn is_supersedeable_status(status: crate::MemoryStatus) -> bool {
    matches!(
        status,
        crate::MemoryStatus::Active | crate::MemoryStatus::Faded | crate::MemoryStatus::Disputed
    )
}

fn merge_hybrid_candidate(
    gathered: &mut std::collections::HashMap<i64, crate::search::GatheredCandidate>,
    user_id: Option<&str>,
    item: crate::MemoryItem,
    vector_similarity: f32,
    source: crate::MemoryCandidateSource,
) {
    use crate::search::{GatheredCandidate, is_recallable_status};

    if !is_recallable_status(item.status) {
        return;
    }
    if let Some(uid) = user_id
        && !item.user_id.is_empty()
        && item.user_id != uid
    {
        return;
    }
    let Some(id) = item.id else {
        return;
    };
    gathered
        .entry(id)
        .and_modify(|candidate| {
            candidate.vector_similarity = candidate.vector_similarity.max(vector_similarity);
            if !candidate.sources.contains(&source) {
                candidate.sources.push(source);
            }
        })
        .or_insert_with(|| GatheredCandidate {
            item,
            vector_similarity,
            sources: vec![source],
        });
}

/// Applies the `SQLite` PRAGMAs the store depends on to the
/// given connection. Idempotent and safe to call from both
/// `open` and `open_in_memory`.
///
/// * `journal_mode=WAL` lets readers proceed concurrently
///   with a writer. WAL is a no-op for in-memory databases
///   (`SQLite` returns `memory`), so it is safe to issue
///   unconditionally.
/// * `busy_timeout=5000` (5 seconds) makes concurrent
///   writers wait for the lock instead of failing with
///   `database is locked` immediately.
/// * `foreign_keys=ON` enables enforcement of foreign-key
///   constraints declared in migrations.
async fn apply_pragmas(db: &DatabaseConnection) -> Result<(), MemoryError> {
    const STATEMENTS: &[&str] = &[
        "PRAGMA journal_mode = WAL",
        "PRAGMA busy_timeout = 5000",
        "PRAGMA foreign_keys = ON",
        "PRAGMA synchronous = NORMAL",
    ];
    for stmt in STATEMENTS {
        db.execute_unprepared(stmt).await.map_err(|e| {
            MemoryError::MemoryStoreConnectionError(format!("failed to apply `{stmt}`: {e}"))
        })?;
    }
    Ok(())
}

async fn list_session_ids_for_card_on_conn<C: ConnectionTrait>(
    conn: &C,
    card_name: &str,
) -> Result<Vec<String>, MemoryError> {
    use sea_orm::QuerySelect;

    let rows = entities::conversation_logs::Entity::find()
        .filter(entities::conversation_logs::Column::CardName.eq(card_name))
        .select_only()
        .column(entities::conversation_logs::Column::SessionId)
        .distinct()
        .into_tuple::<String>()
        .all(conn)
        .await?;
    Ok(rows)
}

impl MemoryStore {
    const fn init(db: DatabaseConnection, embedding_dim: usize) -> Self {
        Self { db, embedding_dim }
    }

    /// Decode stored embedding bytes.
    pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32> {
        bytes_to_embedding(bytes)
    }

    /// Raw `sea-orm` connection — used by migration helpers.
    pub const fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Vector dimensionality for this store's embedding model.
    pub const fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    /// Opens a persistent memory store at the given file path.
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
    pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| MemoryError::MemoryStoreConnectionError("Invalid path".to_string()))?;
        init_sqlite_vec();
        let mut opt = ConnectOptions::new(format!("sqlite:{path_str}?mode=rwc"));
        opt.max_connections(32);
        let db = Database::connect(opt).await?;

        apply_pragmas(&db).await?;

        Migrator::up(&db, None).await?;

        Ok(Self::init(db, embedding_dim))
    }

    /// Opens an in-memory memory store (useful for testing).
    ///
    /// Registers the `sqlite-vec` extension process-globally *before* opening
    /// the connection, since `:memory:` reuses a single persistent connection
    /// for the life of the store. Uses `"sqlite::memory:"` as the database
    /// path with a pool limited to one connection. The same PRAGMAs as
    /// [`open`](Self::open) are applied so behavior matches the file-backed
    /// path.
    pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError> {
        init_sqlite_vec();
        let mut opt = ConnectOptions::new("sqlite::memory:");
        opt.max_connections(1);
        let db = Database::connect(opt).await?;

        apply_pragmas(&db).await?;

        Migrator::up(&db, None).await?;

        Ok(Self::init(db, embedding_dim))
    }

    // ── Conversation Logs ─────────────────────────────────────────────────────

    /// Inserts a conversation log entry.
    pub async fn insert_log(
        &self,
        session_id: &str,
        card_name: &str,
        role: &str,
        content: &str,
    ) -> Result<i64, MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let new_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set(role.to_string()),
            content: Set(content.to_string()),
            created_at: Set(now),
            ..Default::default()
        };

        let res = new_log.insert(&self.db).await?;

        Ok(res.id)
    }

    /// Inserts a full conversation turn (user message + assistant response)
    /// as two log entries in a single transaction.
    pub async fn insert_conversation_turn(
        &self,
        session_id: &str,
        card_name: &str,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<(i64, i64), MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let txn = self.db.begin().await?;
        let user_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set("user".to_string()),
            content: Set(user_message.to_string()),
            created_at: Set(now),
            ..Default::default()
        };
        let user_res = user_log.insert(&txn).await?;

        let assistant_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set("assistant".to_string()),
            content: Set(assistant_response.to_string()),
            created_at: Set(now),
            ..Default::default()
        };
        let assistant_res = assistant_log.insert(&txn).await?;
        txn.commit().await?;

        Ok((user_res.id, assistant_res.id))
    }

    /// Spawns a fire-and-forget task that inserts a conversation log entry.
    ///
    /// Errors are logged at the `error` tracing level. Takes an `Arc<Self>`
    /// so the store outlives the spawned task.
    pub fn spawn_insert_log(
        store: &Arc<Self>,
        session_id: &str,
        card_name: &str,
        role: &str,
        content: &str,
    ) {
        let store = store.clone();
        let session_id = session_id.to_string();
        let card_name = card_name.to_string();
        let role = role.to_string();
        let content = content.to_string();
        tokio::spawn(async move {
            if let Err(e) = store
                .insert_log(&session_id, &card_name, &role, &content)
                .await
            {
                tracing::error!(component = "Memory", role = ?role, error = %e, "Failed to save log");
            }
        });
    }

    /// Returns all conversation logs for a session.
    pub async fn get_logs_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String, DateTime<Utc>)>, MemoryError> {
        let rows = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::SessionId.eq(session_id))
            .order_by_asc(entities::conversation_logs::Column::CreatedAt)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| (row.role, row.content, row.created_at))
            .collect())
    }

    // ── Audit Log (#177) ──────────────────────────────────────────────────────

    /// Records a single audited tool call, redacting sensitive arguments.
    pub async fn insert_audit_entry(
        &self,
        entry: &crate::audit::NewAuditEntry,
    ) -> Result<i64, MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let model = entities::audit_log::ActiveModel {
            turn_id: Set(entry.turn_id.clone()),
            tool_name: Set(entry.tool_name.clone()),
            action: Set(entry.action.clone()),
            target: Set(entry.target.clone()),
            decision: Set(entry.decision.as_str().to_string()),
            success: Set(i32::from(entry.success)),
            redacted_args: Set(crate::audit::redact_arguments(&entry.arguments)),
            created_at: Set(now),
            ..Default::default()
        };

        let res = model.insert(&self.db).await?;
        Ok(res.id)
    }

    /// Spawns a fire-and-forget task that records an audited tool call.
    ///
    /// Errors are logged at the `error` tracing level. Takes an `Arc<Self>`
    /// so the store outlives the spawned task.
    pub fn spawn_insert_audit_entry(store: &Arc<Self>, entry: crate::audit::NewAuditEntry) {
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.insert_audit_entry(&entry).await {
                tracing::error!(
                    component = "AuditLog",
                    tool = %entry.tool_name,
                    error = %e,
                    "Failed to record audit entry"
                );
            }
        });
    }

    /// Returns the most recent audit entries (newest first).
    pub async fn list_audit_entries(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::audit::AuditEntry>, MemoryError> {
        let rows = entities::audit_log::Entity::find()
            .order_by_desc(entities::audit_log::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(audit_row_to_entry).collect())
    }

    /// Returns audit entries filtered by tool name (newest first).
    pub async fn list_audit_entries_by_tool(
        &self,
        tool_name: &str,
        limit: usize,
    ) -> Result<Vec<crate::audit::AuditEntry>, MemoryError> {
        let rows = entities::audit_log::Entity::find()
            .filter(entities::audit_log::Column::ToolName.eq(tool_name))
            .order_by_desc(entities::audit_log::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(audit_row_to_entry).collect())
    }

    // ── Sessions & Export/Import (#176) ─────────────────────────────────────

    /// Inserts a session metadata row, or refreshes `updated_at` if the
    /// `session_id` already exists.
    ///
    /// Only `updated_at` is bumped on conflict; `card_name`, `title`,
    /// `turn_count`, and `archived` are left untouched. Returns the row id.
    pub async fn upsert_session(
        &self,
        meta: &crate::session::NewSessionMeta,
    ) -> Result<i64, MemoryError> {
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let model = entities::session::ActiveModel {
            session_id: Set(meta.session_id.clone()),
            card_name: Set(meta.card_name.clone()),
            title: Set(meta.title.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            archived: Set(0),
            turn_count: Set(0),
            ..Default::default()
        };

        entities::session::Entity::insert(model)
            .on_conflict(
                OnConflict::column(entities::session::Column::SessionId)
                    .update_column(entities::session::Column::UpdatedAt)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        let row = self
            .get_session(&meta.session_id)
            .await?
            .ok_or_else(|| MemoryError::Other("session row missing after upsert".to_string()))?;
        Ok(row.id)
    }

    /// Updates a session's `updated_at` timestamp and `turn_count`.
    ///
    /// No-op if the session does not exist.
    pub async fn touch_session(
        &self,
        session_id: &str,
        turn_count: i64,
    ) -> Result<(), MemoryError> {
        entities::session::Entity::update_many()
            .col_expr(
                entities::session::Column::UpdatedAt,
                Expr::value(Utc::now()),
            )
            .col_expr(
                entities::session::Column::TurnCount,
                Expr::value(turn_count),
            )
            .filter(entities::session::Column::SessionId.eq(session_id))
            .exec(&self.db)
            .await?;

        Ok(())
    }

    /// Returns the metadata for a single session, if present.
    pub async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::session::SessionMeta>, MemoryError> {
        let row = entities::session::Entity::find()
            .filter(entities::session::Column::SessionId.eq(session_id))
            .one(&self.db)
            .await?;

        Ok(row.map(Into::into))
    }

    /// Lists sessions, newest `updated_at` first.
    ///
    /// Archived sessions are excluded unless `include_archived` is set.
    pub async fn list_sessions(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Vec<crate::session::SessionMeta>, MemoryError> {
        let mut query = entities::session::Entity::find();
        if !include_archived {
            query = query.filter(entities::session::Column::Archived.eq(0));
        }
        let rows = query
            .order_by_desc(entities::session::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Sets the archive flag for a session.
    ///
    /// Returns whether a row was actually updated (i.e. the session exists).
    pub async fn set_session_archived(
        &self,
        session_id: &str,
        archived: bool,
    ) -> Result<bool, MemoryError> {
        let res = entities::session::Entity::update_many()
            .col_expr(
                entities::session::Column::Archived,
                Expr::value(i32::from(archived)),
            )
            .filter(entities::session::Column::SessionId.eq(session_id))
            .exec(&self.db)
            .await?;

        Ok(res.rows_affected > 0)
    }

    /// Returns all conversation messages for a session, oldest first.
    pub async fn list_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::export::ExportedMessage>, MemoryError> {
        let rows = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::SessionId.eq(session_id))
            .order_by_asc(entities::conversation_logs::Column::CreatedAt)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(log_row_to_message).collect())
    }

    /// Case-insensitive substring search over conversation message content.
    ///
    /// Returns `(session_id, message)` pairs, paginated by `limit`/`offset`.
    /// The query is bound as a parameter (never concatenated into SQL), so it
    /// is injection-safe. An empty query returns an empty result set.
    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(String, crate::export::ExportedMessage)>, MemoryError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        use sea_orm::ExprTrait;

        let pattern = format!("%{}%", query.to_ascii_lowercase());
        let rows = entities::conversation_logs::Entity::find()
            .filter(
                sea_orm::sea_query::Func::lower(sea_orm::sea_query::Expr::col(
                    entities::conversation_logs::Column::Content,
                ))
                .like(pattern),
            )
            .order_by_desc(entities::conversation_logs::Column::CreatedAt)
            .limit(limit as u64)
            .offset(offset as u64)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let session_id = row.session_id.clone();
                (session_id, log_row_to_message(row))
            })
            .collect())
    }

    /// Assembles a redacted, self-contained export for one session.
    ///
    /// Message content is passed through [`crate::export::redact_secrets`] so
    /// obvious credentials never leave the store in the clear.
    ///
    /// # Limitation
    ///
    /// `tool_logs` is always empty: the `audit_log` table records a `turn_id`
    /// but has no column linking a row back to its session, so audit entries
    /// cannot be associated with a session reliably. Rather than guess (and
    /// risk leaking another session's tool calls or fabricating data), the
    /// export omits tool logs. A future schema revision could add a
    /// `session_id` column to `audit_log` to enable this.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Other`] if the session does not exist.
    pub async fn build_export(
        &self,
        session_id: &str,
    ) -> Result<crate::export::SessionExport, MemoryError> {
        let session = self
            .get_session(session_id)
            .await?
            .ok_or_else(|| MemoryError::Other(format!("session not found: {session_id}")))?;

        let messages = self
            .list_messages(session_id)
            .await?
            .into_iter()
            .map(|mut message| {
                message.content = crate::export::redact_secrets(&message.content);
                message
            })
            .collect();

        Ok(crate::export::SessionExport {
            format_version: crate::export::SESSION_EXPORT_FORMAT_VERSION,
            exported_at: Utc::now(),
            session,
            messages,
            tool_logs: Vec::new(),
        })
    }

    /// Imports a previously exported session, returning the new session row id.
    ///
    /// Conflict handling: if the incoming `session_id` already exists in the
    /// store, the session (and its messages) is imported under a freshly
    /// generated `session_id` (`imported-<uuid>`) so existing sessions are
    /// never overwritten silently. Otherwise the original `session_id` is
    /// preserved.
    ///
    /// # Errors
    ///
    /// Propagates any underlying store error from the inserts.
    pub async fn import_export(
        &self,
        export: &crate::export::SessionExport,
    ) -> Result<i64, MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let target_session_id = if self
            .get_session(&export.session.session_id)
            .await?
            .is_some()
        {
            format!("imported-{}", uuid::Uuid::new_v4())
        } else {
            export.session.session_id.clone()
        };

        let new_meta = crate::session::NewSessionMeta {
            session_id: target_session_id.clone(),
            card_name: export.session.card_name.clone(),
            title: export.session.title.clone(),
        };
        let session_row_id = self.upsert_session(&new_meta).await?;

        let txn = self.db.begin().await?;
        for message in &export.messages {
            let model = entities::conversation_logs::ActiveModel {
                session_id: Set(target_session_id.clone()),
                card_name: Set(new_meta.card_name.clone()),
                role: Set(message.role.clone()),
                content: Set(message.content.clone()),
                created_at: Set(message.created_at),
                ..Default::default()
            };
            model.insert(&txn).await?;
        }
        entities::session::Entity::update_many()
            .col_expr(
                entities::session::Column::TurnCount,
                Expr::value(export.messages.len() as i64),
            )
            .filter(entities::session::Column::SessionId.eq(&target_session_id))
            .exec(&txn)
            .await?;
        txn.commit().await?;

        Ok(session_row_id)
    }

    // ── Tool Embeddings (multi-vector) ──────────────────────────────────────

    /// Inserts or updates one field's embedding for a tool.
    ///
    /// `field` must be one of `"summary"`, `"description"`, `"capability"`,
    /// `"example"`, or `"negative"`, matching the provider's embedding-kind labels.
    /// `field_key` disambiguates multiple entries of the same field type
    /// (e.g. separate `"example"` rows with keys `"ex_0"`, `"ex_1"`).
    pub async fn upsert_tool_embedding_field(
        &self,
        tool_name: &str,
        field: &str,
        field_key: &str,
        version_hash: &str,
        model_name: &str,
        embedding: &[f32],
        source_text: &str,
    ) -> Result<(), MemoryError> {
        use sea_orm::ActiveValue::Set;

        validate_embedding(embedding, self.embedding_dim)?;

        let now = Utc::now();
        let embedding_bytes = embedding_to_bytes(embedding);

        let new_embedding = entities::tool_embedding_index::ActiveModel {
            tool_name: Set(tool_name.to_string()),
            field: Set(field.to_string()),
            field_key: Set(field_key.to_string()),
            version_hash: Set(version_hash.to_string()),
            model_name: Set(model_name.to_string()),
            source_text: Set(source_text.to_string()),
            embedding: Set(embedding_bytes),
            created_at: Set(now),
            ..Default::default()
        };

        entities::tool_embedding_index::Entity::insert(new_embedding)
            .on_conflict(
                OnConflict::columns([
                    entities::tool_embedding_index::Column::ToolName,
                    entities::tool_embedding_index::Column::Field,
                    entities::tool_embedding_index::Column::FieldKey,
                    entities::tool_embedding_index::Column::ModelName,
                ])
                .update_columns([
                    entities::tool_embedding_index::Column::VersionHash,
                    entities::tool_embedding_index::Column::Embedding,
                    entities::tool_embedding_index::Column::SourceText,
                    entities::tool_embedding_index::Column::CreatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    /// Lists all stored tool embeddings, one row per (`tool_name`, field, `field_key`, `model_name`).
    pub async fn list_tool_embedding_fields(
        &self,
    ) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError> {
        let rows = entities::tool_embedding_index::Entity::find()
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.tool_name,
                    row.field,
                    row.field_key,
                    row.version_hash,
                    row.model_name,
                    bytes_to_embedding(&row.embedding),
                    row.source_text,
                )
            })
            .collect())
    }

    /// Returns `(tool_name, field, field_key, version_hash, model_name)`
    /// for every cached tool embedding row, **without**
    /// deserializing the vector or fetching the source
    /// text. Used by Tool RAG's `ensure_index` to decide
    /// which fields are up-to-date; the previous form
    /// deserialized every f32 vector on every turn and
    /// then discarded them.
    pub async fn list_tool_embedding_hashes(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, MemoryError> {
        let rows = entities::tool_embedding_index::Entity::find()
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.tool_name,
                    row.field,
                    row.field_key,
                    row.version_hash,
                    row.model_name,
                )
            })
            .collect())
    }

    /// Deletes all field embeddings for a tool (cascades across all fields).
    pub async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError> {
        let res = entities::tool_embedding_index::Entity::delete_many()
            .filter(entities::tool_embedding_index::Column::ToolName.eq(tool_name))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected as usize)
    }

    /// Searches tool embeddings by cosine similarity to the query across ALL
    /// fields, then aggregates the per-field similarity scores for each tool
    /// using max-pool (the strongest signal wins). Returns tools sorted by
    /// aggregated similarity.
    pub async fn search_tools(
        &self,
        query_embedding: &[f32],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(String, f32)>, MemoryError> {
        #[derive(Debug, FromQueryResult)]
        struct SearchToolRow {
            tool_name: String,
            similarity: f64,
        }

        validate_embedding(query_embedding, self.embedding_dim)?;

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = cosine_similarity_expr("embedding", &query_bytes);

        let factor = 4u64;
        let row_cap = (limit as u64).saturating_mul(factor).max(limit as u64);

        let select = entities::tool_embedding_index::Entity::find()
            .select_only()
            .column(entities::tool_embedding_index::Column::ToolName)
            .expr_as(similarity_expr, "similarity")
            .filter(cosine_similarity_filter(
                "embedding",
                &query_bytes,
                f64::from(similarity_threshold),
            ))
            .order_by_desc(Expr::col("similarity"))
            .limit(row_cap);

        let rows = select.into_model::<SearchToolRow>().all(&self.db).await?;

        use std::collections::HashMap;
        let mut by_tool: HashMap<String, f32> = HashMap::new();
        for row in rows {
            let sim = row.similarity as f32;
            let entry = by_tool.entry(row.tool_name).or_insert(f32::MIN);
            if sim > *entry {
                *entry = sim;
            }
        }

        let mut aggregated: Vec<(String, f32)> = by_tool.into_iter().collect();
        aggregated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        aggregated.truncate(limit);

        Ok(aggregated)
    }

    /// Retrieve the current [`crate::AffectState`] for a character.
    pub async fn get_affect_state(
        &self,
        character_id: &str,
    ) -> Result<crate::AffectState, MemoryError> {
        use entities::affect_states::Entity;
        use sea_orm::EntityTrait;

        let maybe_model = Entity::find_by_id(character_id).one(&self.db).await?;
        match maybe_model {
            Some(model) => {
                let discrete_emotions: Vec<crate::DiscreteEmotion> =
                    serde_json::from_str(&model.discrete_emotions).unwrap_or_else(|e| {
                        tracing::error!(
                            component = "MemoryStore",
                            character_id = %model.character_id,
                            error = %e,
                            "Failed to deserialize discrete_emotions, returning empty list"
                        );
                        Vec::new()
                    });
                Ok(crate::AffectState {
                    character_id: model.character_id,
                    user_id: model.user_id,
                    valence: model.valence,
                    arousal: model.arousal,
                    dominance: model.dominance,
                    trust: model.trust,
                    affinity: model.affinity,
                    irritation: model.irritation,
                    curiosity: model.curiosity,
                    fatigue: model.fatigue,
                    mood_label: model.mood_label,
                    last_expression: model.last_expression,
                    discrete_emotions,
                    updated_at: Some(model.updated_at),
                })
            }
            None => Ok(crate::AffectState::neutral(character_id)),
        }
    }

    /// Persist or update an [`crate::AffectState`].
    pub async fn upsert_affect_state(&self, state: &crate::AffectState) -> Result<(), MemoryError> {
        use entities::affect_states::{ActiveModel, Column, Entity};
        use sea_orm::sea_query::OnConflict;

        let mut state = state.clone();
        state.clamp();

        let now = Utc::now();
        let discrete_json = serde_json::to_string(&state.discrete_emotions)
            .map_err(|e| MemoryError::Other(e.to_string()))?;

        let active = ActiveModel {
            character_id: sea_orm::Set(state.character_id),
            user_id: sea_orm::Set(state.user_id),
            valence: sea_orm::Set(state.valence),
            arousal: sea_orm::Set(state.arousal),
            dominance: sea_orm::Set(state.dominance),
            trust: sea_orm::Set(state.trust),
            affinity: sea_orm::Set(state.affinity),
            irritation: sea_orm::Set(state.irritation),
            curiosity: sea_orm::Set(state.curiosity),
            fatigue: sea_orm::Set(state.fatigue),
            mood_label: sea_orm::Set(state.mood_label),
            last_expression: sea_orm::Set(state.last_expression),
            discrete_emotions: sea_orm::Set(discrete_json),
            updated_at: sea_orm::Set(now),
        };

        Entity::insert(active)
            .on_conflict(
                OnConflict::column(Column::CharacterId)
                    .update_columns([
                        Column::UserId,
                        Column::Valence,
                        Column::Arousal,
                        Column::Dominance,
                        Column::Trust,
                        Column::Affinity,
                        Column::Irritation,
                        Column::Curiosity,
                        Column::Fatigue,
                        Column::MoodLabel,
                        Column::LastExpression,
                        Column::DiscreteEmotions,
                        Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    /// Upsert a pending post-turn classifier proposal for the next turn.
    pub async fn upsert_pending_affect_proposal(
        &self,
        proposal: &crate::PendingAffectProposal,
    ) -> Result<(), MemoryError> {
        use entities::pending_affect_proposals::{ActiveModel, Column, Entity};
        use sea_orm::sea_query::OnConflict;

        let proposal_json =
            serde_json::to_string(proposal).map_err(|e| MemoryError::Other(e.to_string()))?;

        let active = ActiveModel {
            character_id: sea_orm::Set(proposal.character_id.clone()),
            user_id: sea_orm::Set(proposal.user_id.clone()),
            source_turn_id: sea_orm::Set(proposal.source_turn_id),
            proposal_json: sea_orm::Set(proposal_json),
            created_at: sea_orm::Set(proposal.created_at),
        };

        Entity::insert(active)
            .on_conflict(
                OnConflict::columns([Column::CharacterId, Column::UserId])
                    .update_columns([
                        Column::SourceTurnId,
                        Column::ProposalJson,
                        Column::CreatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Retrieve a pending post-turn classifier proposal, if any.
    pub async fn get_pending_affect_proposal(
        &self,
        character_id: &str,
        user_id: &str,
    ) -> Result<Option<crate::PendingAffectProposal>, MemoryError> {
        use entities::pending_affect_proposals::Entity;
        use sea_orm::EntityTrait;

        let maybe_model = Entity::find_by_id((character_id.to_string(), user_id.to_string()))
            .one(&self.db)
            .await?;
        let Some(model) = maybe_model else {
            return Ok(None);
        };

        match serde_json::from_str::<crate::PendingAffectProposal>(&model.proposal_json) {
            Ok(proposal) => Ok(Some(proposal)),
            Err(error) => {
                tracing::warn!(
                    component = "MemoryStore",
                    character_id,
                    user_id,
                    error = %error,
                    "Dropping stale pending affect proposal with incompatible JSON"
                );
                self.delete_pending_affect_proposal(character_id, user_id)
                    .await?;
                Ok(None)
            }
        }
    }

    /// Delete a pending post-turn classifier proposal for a character/user key.
    pub async fn delete_pending_affect_proposal(
        &self,
        character_id: &str,
        user_id: &str,
    ) -> Result<(), MemoryError> {
        use entities::pending_affect_proposals::Entity;
        use sea_orm::EntityTrait;

        Entity::delete_by_id((character_id.to_string(), user_id.to_string()))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Fetch and consume a pending post-turn classifier proposal.
    pub async fn take_pending_affect_proposal(
        &self,
        character_id: &str,
        user_id: &str,
    ) -> Result<Option<crate::PendingAffectProposal>, MemoryError> {
        let proposal = self
            .get_pending_affect_proposal(character_id, user_id)
            .await?;
        if proposal.is_some() {
            self.delete_pending_affect_proposal(character_id, user_id)
                .await?;
        }
        Ok(proposal)
    }

    // ── Typed Memory CRUD ───────────────────────────────────────────────────

    /// Insert a new typed memory item and return its assigned ID.
    pub async fn insert_typed_memory(
        &self,
        item: &crate::NewMemoryItem,
    ) -> Result<i64, MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let created_at = item.created_at.unwrap_or(now);
        let active = entities::typed_memories::ActiveModel {
            scope: Set(item.scope.as_str().to_string()),
            character_id: Set(item.character_id.clone()),
            user_id: Set(item.user_id.clone()),
            kind: Set(item.kind.as_str().to_string()),
            title: Set(item.title.clone()),
            content: Set(item.content.clone()),
            source: Set(item.source.as_str().to_string()),
            source_ref: Set(item.source_ref.clone()),
            confidence: Set(item.confidence.get()),
            salience: Set(item.salience.get()),
            affective_valence: Set(item.affect.valence),
            affective_arousal: Set(item.affect.arousal),
            relationship_impact: Set(item.relationship_impact),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(created_at),
            updated_at: Set(now),
            valid_from: Set(item.valid_from),
            valid_until: Set(item.valid_until),
            status: Set(item.status.as_str().to_string()),
            supersedes_id: Set(item.supersedes_id),
            pinned: Set(i32::from(item.pinned)),
            commitment_id: Set(item.commitment_id),
            ..Default::default()
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// Retrieve a typed memory item by its ID.
    pub async fn get_typed_memory(
        &self,
        id: i64,
    ) -> Result<Option<crate::MemoryItem>, MemoryError> {
        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        match maybe_model {
            Some(m) => model_to_memory_item(m).map(Some),
            None => Ok(None),
        }
    }

    /// List typed memories for a character, optionally filtered by kind.
    pub async fn get_typed_memories_by_character(
        &self,
        character_id: &str,
        kind: Option<crate::MemoryKind>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id));

        if let Some(k) = kind {
            query = query.filter(entities::typed_memories::Column::Kind.eq(k.as_str()));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::CreatedAt)
            .limit(limit as u64)
            .offset(offset as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Count typed memories for a character, optionally filtered by kind.
    pub async fn count_typed_memories(
        &self,
        character_id: &str,
        kind: Option<crate::MemoryKind>,
    ) -> Result<i64, MemoryError> {
        use sea_orm::{EntityTrait, PaginatorTrait, QueryFilter};

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id));

        if let Some(k) = kind {
            query = query.filter(entities::typed_memories::Column::Kind.eq(k.as_str()));
        }

        Ok(query.count(&self.db).await? as i64)
    }

    /// List active typed memories whose `source_ref` starts with `prefix`.
    pub async fn list_typed_memories_by_source_prefix(
        &self,
        character_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let models = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(
                entities::typed_memories::Column::Status.eq(crate::MemoryStatus::Active.as_str()),
            )
            .filter(entities::typed_memories::Column::SourceRef.starts_with(prefix))
            .order_by_desc(entities::typed_memories::Column::Salience)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Returns whether an active typed memory exists for `character_id` + `source_ref`.
    pub async fn typed_memory_exists_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<bool, MemoryError> {
        Ok(self
            .get_active_typed_memory_by_source_ref(character_id, source_ref)
            .await?
            .is_some())
    }

    /// Returns the active typed memory for `character_id` + `source_ref`, if any.
    pub async fn get_active_typed_memory_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<Option<crate::MemoryItem>, MemoryError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};

        let model = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::SourceRef.eq(source_ref))
            .filter(
                entities::typed_memories::Column::Status.eq(crate::MemoryStatus::Active.as_str()),
            )
            .limit(1)
            .one(&self.db)
            .await?;

        match model {
            Some(m) => model_to_memory_item(m).map(Some),
            None => Ok(None),
        }
    }

    /// Archive active typed memories under `prefixes` whose `source_ref` is not kept.
    pub async fn archive_typed_memories_by_source_prefixes(
        &self,
        character_id: &str,
        prefixes: &[&str],
        keep_refs: &std::collections::HashSet<String>,
    ) -> Result<usize, MemoryError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

        let mut archived = 0usize;
        for prefix in prefixes {
            let models = entities::typed_memories::Entity::find()
                .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
                .filter(
                    entities::typed_memories::Column::Status
                        .eq(crate::MemoryStatus::Active.as_str()),
                )
                .filter(entities::typed_memories::Column::SourceRef.starts_with(*prefix))
                .all(&self.db)
                .await?;

            for model in models {
                let Some(source_ref) = model.source_ref else {
                    continue;
                };
                if keep_refs.contains(&source_ref) {
                    continue;
                }
                self.set_memory_status(model.id, crate::MemoryStatus::Faded)
                    .await?;
                self.set_memory_status(model.id, crate::MemoryStatus::Archived)
                    .await?;
                archived = archived.saturating_add(1);
            }
        }
        Ok(archived)
    }

    /// Search typed memories with explainable hybrid scoring (#123).
    ///
    /// Sole public typed-memory search entry. Gathers candidates from optional
    /// vector similarity (when `query.embedding` is `Some`), lexical token
    /// matches, a limited recent fallback, and active commitments; scores and
    /// de-duplicates by memory id; returns the top `query.limit` results.
    /// Callers must pre-compute embeddings — the store never embeds.
    pub async fn search(
        &self,
        query: &crate::Query<'_>,
    ) -> Result<Vec<crate::ScoredMemory>, MemoryError> {
        use crate::search::{lexical_overlap_score, score_candidate};
        use crate::typed_memory::MemoryCandidateSource;
        use std::collections::HashMap;

        if let Some(embedding) = query.embedding {
            validate_embedding(embedding, self.embedding_dim)?;
        }

        let pool = query.candidate_pool_size.max(query.limit);
        let mut gathered: HashMap<i64, crate::search::GatheredCandidate> = HashMap::new();

        // Vector candidates across recallable statuses (skipped when no embedding).
        if let Some(embedding) = query.embedding {
            let vector_hits = self
                .search_typed_memories_vector(
                    embedding,
                    query.character_id,
                    query.model_name,
                    query.user_id,
                    &RECALLABLE_STATUSES,
                    pool,
                    query.similarity_threshold,
                )
                .await?;
            for (item, similarity) in vector_hits {
                merge_hybrid_candidate(
                    &mut gathered,
                    query.user_id,
                    item,
                    similarity,
                    MemoryCandidateSource::Vector,
                );
            }
        }

        // Lexical candidates from token-based DB lookup.
        let lexical_candidates = self
            .list_lexical_typed_memory_candidates(
                query.query_text,
                query.character_id,
                query.user_id,
                pool,
            )
            .await?;
        for item in lexical_candidates {
            let lexical = lexical_overlap_score(query.query_text, &item.title, &item.content);
            if lexical > 0.0 {
                merge_hybrid_candidate(
                    &mut gathered,
                    query.user_id,
                    item,
                    0.0,
                    MemoryCandidateSource::Lexical,
                );
            }
        }

        // Active commitment ledger rows are the SoT for Commitment boost (#124).
        // Prefer typed rows linked via commitment_id; otherwise synthesize from ledger.
        let commitments = self
            .list_active_commitments(query.character_id, query.user_id, pool)
            .await?;
        let commitment_ids: Vec<i64> = commitments.iter().filter_map(|c| c.id).collect();
        let linked = self
            .get_typed_memories_by_commitment_ids(&commitment_ids)
            .await?;
        let mut linked_commitment_ids = std::collections::HashSet::new();
        for item in linked {
            if let Some(cid) = item.commitment_id {
                linked_commitment_ids.insert(cid);
            }
            merge_hybrid_candidate(
                &mut gathered,
                query.user_id,
                item,
                0.0,
                MemoryCandidateSource::Commitment,
            );
        }
        for commitment in &commitments {
            let Some(cid) = commitment.id else {
                continue;
            };
            if linked_commitment_ids.contains(&cid) {
                continue;
            }
            // Ephemeral MemoryItem keyed by negated commitment id to avoid
            // colliding with typed_memories primary keys in the gather map.
            let item = crate::MemoryItem {
                id: Some(0_i64.wrapping_sub(cid)),
                scope: crate::MemoryScope::Character,
                character_id: commitment.character_id.clone(),
                user_id: commitment.user_id.clone(),
                kind: crate::MemoryKind::Commitment,
                title: commitment.title.clone(),
                content: commitment.description.clone(),
                source: crate::MemorySource::Conversation,
                source_ref: Some(format!("commitment:{cid}")),
                confidence: crate::MemoryConfidence::new(0.9),
                salience: crate::MemorySalience::new(0.8),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: commitment.created_at,
                updated_at: commitment.updated_at,
                valid_from: None,
                valid_until: commitment.due_at,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: Some(cid),
            };
            merge_hybrid_candidate(
                &mut gathered,
                query.user_id,
                item,
                0.0,
                MemoryCandidateSource::Commitment,
            );
        }

        // Limited recent fallback for memories not already gathered.
        if query.recent_fallback_limit > 0 {
            let recent_candidates = self
                .list_recallable_typed_memories(
                    query.character_id,
                    query.user_id,
                    query.recent_fallback_limit.saturating_mul(2).max(pool),
                )
                .await?;
            let mut recent_added = 0usize;
            for item in recent_candidates {
                if recent_added >= query.recent_fallback_limit {
                    break;
                }
                let Some(id) = item.id else {
                    continue;
                };
                if gathered.contains_key(&id) {
                    continue;
                }
                merge_hybrid_candidate(
                    &mut gathered,
                    query.user_id,
                    item,
                    0.0,
                    MemoryCandidateSource::Recent,
                );
                recent_added = recent_added.saturating_add(1);
            }
        }

        let mut scored: Vec<crate::ScoredMemory> = gathered
            .into_values()
            .map(|candidate| {
                let breakdown = score_candidate(query, &candidate);
                crate::ScoredMemory {
                    item: candidate.item,
                    breakdown,
                    sources: candidate.sources,
                }
            })
            .filter(|scored| scored.breakdown.total >= query.min_score)
            .collect();

        scored.sort_by(|a, b| {
            b.breakdown
                .total
                .total_cmp(&a.breakdown.total)
                .then_with(|| {
                    b.breakdown
                        .vector_similarity
                        .total_cmp(&a.breakdown.vector_similarity)
                })
                .then_with(|| b.item.updated_at.cmp(&a.item.updated_at))
        });

        if scored.len() > query.limit {
            scored.truncate(query.limit);
        }
        Ok(scored)
    }

    /// Legacy vector-only search over `active` memories (tests / internal).
    #[cfg(test)]
    pub(crate) async fn search_typed_memories(
        &self,
        query_embedding: &[f32],
        character_id: &str,
        model_name: &str,
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(crate::MemoryItem, f32)>, MemoryError> {
        self.search_typed_memories_vector(
            query_embedding,
            character_id,
            model_name,
            None,
            &[crate::MemoryStatus::Active.as_str()],
            limit,
            similarity_threshold,
        )
        .await
    }

    /// Vector similarity search with configurable recallable statuses.
    async fn search_typed_memories_vector(
        &self,
        query_embedding: &[f32],
        character_id: &str,
        model_name: &str,
        user_id: Option<&str>,
        statuses: &[&str],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(crate::MemoryItem, f32)>, MemoryError> {
        #[derive(Debug, FromQueryResult)]
        struct SearchMemoryRow {
            id: i64,
            scope: String,
            character_id: String,
            user_id: String,
            kind: String,
            title: String,
            content: String,
            source: String,
            source_ref: Option<String>,
            confidence: f32,
            salience: f32,
            affective_valence: f32,
            affective_arousal: f32,
            relationship_impact: f32,
            access_count: i64,
            last_accessed_at: Option<DateTime<Utc>>,
            created_at: DateTime<Utc>,
            updated_at: DateTime<Utc>,
            valid_from: Option<DateTime<Utc>>,
            valid_until: Option<DateTime<Utc>>,
            status: String,
            supersedes_id: Option<i64>,
            pinned: i32,
            faded_at: Option<DateTime<Utc>>,
            commitment_id: Option<i64>,
            similarity: f64,
        }

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = cosine_similarity_expr("memory_embeddings.embedding", &query_bytes);

        validate_embedding(query_embedding, self.embedding_dim)?;

        let threshold_val = f64::from(similarity_threshold);
        let limit_val = limit as u64;

        let mut select = entities::memory_embeddings::Entity::find()
            .inner_join(entities::typed_memories::Entity)
            .select_only()
            .column(entities::typed_memories::Column::Id)
            .column(entities::typed_memories::Column::Scope)
            .column(entities::typed_memories::Column::CharacterId)
            .column(entities::typed_memories::Column::UserId)
            .column(entities::typed_memories::Column::Kind)
            .column(entities::typed_memories::Column::Title)
            .column(entities::typed_memories::Column::Content)
            .column(entities::typed_memories::Column::Source)
            .column(entities::typed_memories::Column::SourceRef)
            .column(entities::typed_memories::Column::Confidence)
            .column(entities::typed_memories::Column::Salience)
            .column(entities::typed_memories::Column::AffectiveValence)
            .column(entities::typed_memories::Column::AffectiveArousal)
            .column(entities::typed_memories::Column::RelationshipImpact)
            .column(entities::typed_memories::Column::AccessCount)
            .column(entities::typed_memories::Column::LastAccessedAt)
            .column(entities::typed_memories::Column::CreatedAt)
            .column(entities::typed_memories::Column::UpdatedAt)
            .column(entities::typed_memories::Column::ValidFrom)
            .column(entities::typed_memories::Column::ValidUntil)
            .column(entities::typed_memories::Column::Status)
            .column(entities::typed_memories::Column::SupersedesId)
            .column(entities::typed_memories::Column::Pinned)
            .column(entities::typed_memories::Column::FadedAt)
            .column(entities::typed_memories::Column::CommitmentId)
            .expr_as(similarity_expr, "similarity")
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(statuses.to_vec()))
            .filter(entities::memory_embeddings::Column::ModelName.eq(model_name))
            .filter(entities::memory_embeddings::Column::Field.eq("content"))
            .filter(cosine_similarity_filter(
                "memory_embeddings.embedding",
                &query_bytes,
                threshold_val,
            ))
            .order_by_desc(Expr::col("similarity"))
            .limit(limit_val);

        if let Some(uid) = user_id {
            select = select.filter(user_visibility_condition(uid));
        }

        let rows = select.into_model::<SearchMemoryRow>().all(&self.db).await?;

        rows.into_iter()
            .map(|row| {
                Ok((
                    crate::MemoryItem {
                        id: Some(row.id),
                        scope: str_to_scope(&row.scope),
                        character_id: row.character_id,
                        user_id: row.user_id,
                        kind: str_to_kind(&row.kind),
                        title: row.title,
                        content: row.content,
                        source: str_to_source(&row.source),
                        source_ref: row.source_ref,
                        confidence: crate::MemoryConfidence::new(row.confidence),
                        salience: crate::MemorySalience::new(row.salience),
                        affect: crate::AffectAnnotation {
                            valence: row.affective_valence,
                            arousal: row.affective_arousal,
                        },
                        relationship_impact: row.relationship_impact,
                        access_count: row.access_count,
                        last_accessed_at: row.last_accessed_at,
                        created_at: row.created_at,
                        updated_at: row.updated_at,
                        valid_from: row.valid_from,
                        valid_until: row.valid_until,
                        status: str_to_status(&row.status),
                        supersedes_id: row.supersedes_id,
                        pinned: row.pinned != 0,
                        faded_at: row.faded_at,
                        commitment_id: row.commitment_id,
                    },
                    row.similarity as f32,
                ))
            })
            .collect()
    }

    /// List typed memories eligible for hybrid recall (`active`, `faded`, `disputed`).
    pub async fn list_recallable_typed_memories(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(RECALLABLE_STATUSES));

        if let Some(uid) = user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Fetch typed memories linked to commitment ledger rows (#124).
    async fn get_typed_memories_by_commitment_ids(
        &self,
        commitment_ids: &[i64],
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter};

        if commitment_ids.is_empty() {
            return Ok(Vec::new());
        }

        let models = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CommitmentId.is_in(commitment_ids.to_vec()))
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// List recallable typed memories whose title or content matches query tokens.
    async fn list_lexical_typed_memory_candidates(
        &self,
        query_text: &str,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use crate::search::tokenize;
        use sea_orm::{Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let tokens: Vec<String> = tokenize(query_text).into_iter().collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let mut lexical_match = Condition::any();
        for token in tokens {
            let pattern = format!("%{token}%");
            lexical_match = lexical_match
                .add(entities::typed_memories::Column::Title.like(&pattern))
                .add(entities::typed_memories::Column::Content.like(&pattern));
        }

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(RECALLABLE_STATUSES))
            .filter(lexical_match);

        if let Some(uid) = user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Atomically insert a replacement memory and mark the prior row superseded.
    ///
    /// The new row's `supersedes_id` is set to `superseded_id` (predecessor link).
    /// The old row is transitioned to [`crate::MemoryStatus::Superseded`] with
    /// `supersedes_id` cleared. Only rows in `Active`, `Faded`, or `Disputed`
    /// status may be superseded.
    pub async fn supersede_typed_memory(
        &self,
        new_item: &crate::NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, TransactionTrait};

        let txn = self.db.begin().await?;

        let old_model = entities::typed_memories::Entity::find_by_id(superseded_id)
            .one(&txn)
            .await?
            .ok_or_else(|| {
                MemoryError::Other(format!("superseded memory id={superseded_id} not found"))
            })?;

        let old_status = str_to_status(&old_model.status);
        if !is_supersedeable_status(old_status) {
            return Err(MemoryError::Other(format!(
                "memory id={superseded_id} cannot be superseded (status={})",
                old_model.status
            )));
        }

        let now = Utc::now();
        let mut insert_item = new_item.clone();
        insert_item.supersedes_id = Some(superseded_id);

        let active = entities::typed_memories::ActiveModel {
            scope: Set(insert_item.scope.as_str().to_string()),
            character_id: Set(insert_item.character_id.clone()),
            user_id: Set(insert_item.user_id.clone()),
            kind: Set(insert_item.kind.as_str().to_string()),
            title: Set(insert_item.title.clone()),
            content: Set(insert_item.content.clone()),
            source: Set(insert_item.source.as_str().to_string()),
            source_ref: Set(insert_item.source_ref.clone()),
            confidence: Set(insert_item.confidence.get()),
            salience: Set(insert_item.salience.get()),
            affective_valence: Set(insert_item.affect.valence),
            affective_arousal: Set(insert_item.affect.arousal),
            relationship_impact: Set(insert_item.relationship_impact),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            valid_from: Set(insert_item.valid_from),
            valid_until: Set(insert_item.valid_until),
            status: Set(insert_item.status.as_str().to_string()),
            supersedes_id: Set(insert_item.supersedes_id),
            pinned: Set(i32::from(insert_item.pinned)),
            ..Default::default()
        };
        let inserted = active.insert(&txn).await?;
        let new_id = inserted.id;

        let mut old_active: entities::typed_memories::ActiveModel = old_model.into();
        old_active.status = Set(crate::MemoryStatus::Superseded.as_str().to_string());
        old_active.supersedes_id = Set(None);
        old_active.updated_at = Set(now);
        old_active.update(&txn).await?;

        txn.commit().await?;
        Ok(new_id)
    }

    /// Bump the access count and last-accessed timestamp for a typed memory.
    pub async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryError> {
        use sea_orm::ExprTrait;

        let now = Utc::now();
        let result = entities::typed_memories::Entity::update_many()
            .col_expr(
                entities::typed_memories::Column::AccessCount,
                Expr::col(entities::typed_memories::Column::AccessCount).add(1),
            )
            .col_expr(
                entities::typed_memories::Column::LastAccessedAt,
                Expr::value(now),
            )
            .filter(entities::typed_memories::Column::Id.eq(id))
            .exec(&self.db)
            .await?;
        Ok(result.rows_affected > 0)
    }

    /// Transition a typed memory with lifecycle edge validation (#76).
    pub async fn set_memory_status(
        &self,
        id: i64,
        new_status: crate::MemoryStatus,
    ) -> Result<bool, MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let current = str_to_status(&model.status);
        if let Err(invalid) = crate::forgetting::validate_transition(current, new_status) {
            return Err(MemoryError::InvalidTransition {
                from: invalid.from,
                to: invalid.to,
            });
        }

        let item = model_to_memory_item(model.clone())?;
        let now = Utc::now();
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.status = Set(new_status.as_str().to_string());
        active.updated_at = Set(now);
        if current == crate::MemoryStatus::Active && new_status == crate::MemoryStatus::Faded {
            active.faded_at = Set(Some(crate::forgetting::active_decay_anchor(&item)));
        }
        active.update(&self.db).await?;
        Ok(true)
    }

    /// User-driven restore to [`MemoryStatus::Active`] (journal/CLI UX).
    pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let current = str_to_status(&model.status);
        if let Err(invalid) = crate::forgetting::validate_user_restore(current) {
            return Err(MemoryError::InvalidTransition {
                from: invalid.from,
                to: invalid.to,
            });
        }

        let now = Utc::now();
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.status = Set(crate::MemoryStatus::Active.as_str().to_string());
        active.faded_at = Set(None);
        active.updated_at = Set(now);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// User-driven forget (`Active` → `UserDeleted`).
    pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, MemoryError> {
        self.set_memory_status(id, crate::MemoryStatus::UserDeleted)
            .await
    }

    /// List typed memories for the memory journal with user/scope and status filters.
    pub async fn list_journal_memories(
        &self,
        options: &crate::MemoryJournalListOptions<'_>,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut allowed_statuses = RECALLABLE_STATUSES.to_vec();
        if options.include_archived {
            allowed_statuses.push(crate::MemoryStatus::Archived.as_str());
        }
        if options.include_superseded {
            allowed_statuses.push(crate::MemoryStatus::Superseded.as_str());
        }
        if options.include_user_deleted {
            allowed_statuses.push(crate::MemoryStatus::UserDeleted.as_str());
        }

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(options.character_id))
            .filter(entities::typed_memories::Column::Status.is_in(allowed_statuses));

        if let Some(uid) = options.user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        if let Some(kind) = options.kind {
            query = query.filter(entities::typed_memories::Column::Kind.eq(kind.as_str()));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::UpdatedAt)
            .limit(options.limit as u64)
            .offset(options.offset as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Set whether a typed memory is pinned (exempt from natural decay).
    pub async fn pin_typed_memory(&self, id: i64, pinned: bool) -> Result<bool, MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let now = Utc::now();
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.pinned = Set(i32::from(pinned));
        active.updated_at = Set(now);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// List typed memories eligible for natural decay processing.
    pub async fn list_memories_for_decay(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        statuses: &[crate::MemoryStatus],
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        if statuses.is_empty() {
            return Ok(vec![]);
        }

        let status_strs: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(status_strs));

        if let Some(uid) = user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        let models = query
            .order_by_asc(entities::typed_memories::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Apply natural decay transitions for recallable memories in a scope.
    pub async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: DateTime<Utc>,
        half_life_days: f64,
        limit: usize,
    ) -> Result<NaturalDecayReport, MemoryError> {
        let candidates = self
            .list_memories_for_decay(
                character_id,
                user_id,
                &[crate::MemoryStatus::Active, crate::MemoryStatus::Faded],
                limit,
            )
            .await?;

        let mut report = NaturalDecayReport::default();
        for item in candidates {
            let Some(id) = item.id else {
                continue;
            };
            if item.pinned {
                continue;
            }
            let score = crate::forgetting::decay_score(&item, now, half_life_days);
            let Some(target) = crate::forgetting::target_status_after_decay(item.status, score)
            else {
                continue;
            };
            self.set_memory_status(id, target).await?;
            match target {
                crate::MemoryStatus::Faded => {
                    report.faded_count = report.faded_count.saturating_add(1);
                }
                crate::MemoryStatus::Archived => {
                    report.archived_count = report.archived_count.saturating_add(1);
                }
                _ => {}
            }
        }
        Ok(report)
    }

    /// Backdate typed memory timestamps for integration tests (#76).
    #[doc(hidden)]
    pub async fn test_backdate_typed_memory(
        &self,
        id: i64,
        days_ago: i64,
    ) -> Result<bool, MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        let Some(model) = maybe_model else {
            return Ok(false);
        };
        let anchor = Utc::now()
            .checked_sub_signed(chrono::Duration::days(days_ago))
            .unwrap_or_else(Utc::now);
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.created_at = Set(anchor);
        active.updated_at = Set(anchor);
        active.last_accessed_at = Set(None);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// Store a content embedding for a typed memory item.
    pub async fn upsert_memory_embedding(
        &self,
        memory_item_id: i64,
        model_name: &str,
        field: &str,
        embedding: &[f32],
    ) -> Result<(), MemoryError> {
        use sea_orm::sea_query::OnConflict;
        use sea_orm::{ActiveValue::Set, EntityTrait};

        validate_embedding(embedding, self.embedding_dim)?;

        let now = Utc::now();
        let embedding_bytes = embedding_to_bytes(embedding);

        let active = entities::memory_embeddings::ActiveModel {
            memory_item_id: Set(memory_item_id),
            model_name: Set(model_name.to_string()),
            field: Set(field.to_string()),
            embedding: Set(embedding_bytes),
            created_at: Set(now),
            ..Default::default()
        };

        entities::memory_embeddings::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    entities::memory_embeddings::Column::MemoryItemId,
                    entities::memory_embeddings::Column::ModelName,
                    entities::memory_embeddings::Column::Field,
                ])
                .update_column(entities::memory_embeddings::Column::Embedding)
                .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    // ── Memory Spans ────────────────────────────────────────────────────────

    /// Distinct session IDs that have conversation logs for a character card.
    pub async fn list_session_ids_for_card(
        &self,
        card_name: &str,
    ) -> Result<Vec<String>, MemoryError> {
        list_session_ids_for_card_on_conn(&self.db, card_name).await
    }

    /// Returns true when a memory span already exists for the session turn.
    pub async fn memory_span_exists(
        &self,
        session_id: &str,
        turn_start: i32,
    ) -> Result<bool, MemoryError> {
        use sea_orm::PaginatorTrait;

        let count = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .filter(entities::memory_spans::Column::TurnStart.eq(turn_start))
            .count(&self.db)
            .await?;
        Ok(count > 0)
    }

    /// Insert a memory span row.
    pub async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let active = entities::memory_spans::ActiveModel {
            session_id: Set(span.session_id.clone()),
            turn_start: Set(span.turn_start),
            turn_end: Set(span.turn_end),
            raw_excerpt: Set(span.raw_excerpt.clone()),
            compressed_summary: Set(span.compressed_summary.clone()),
            compression_level: Set(span.compression_level),
            ..Default::default()
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// List memory spans for a session ordered by turn start.
    pub async fn list_memory_spans_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<NewMemorySpan>, MemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .order_by_asc(entities::memory_spans::Column::TurnStart)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| NewMemorySpan {
                session_id: r.session_id,
                turn_start: r.turn_start,
                turn_end: r.turn_end,
                raw_excerpt: r.raw_excerpt,
                compressed_summary: r.compressed_summary,
                compression_level: r.compression_level,
            })
            .collect())
    }

    /// List memory spans for a session filtered by compression level.
    pub async fn list_memory_spans_by_session_and_level(
        &self,
        session_id: &str,
        compression_level: i32,
    ) -> Result<Vec<NewMemorySpan>, MemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .filter(entities::memory_spans::Column::CompressionLevel.eq(compression_level))
            .order_by_asc(entities::memory_spans::Column::TurnStart)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| NewMemorySpan {
                session_id: r.session_id,
                turn_start: r.turn_start,
                turn_end: r.turn_end,
                raw_excerpt: r.raw_excerpt,
                compressed_summary: r.compressed_summary,
                compression_level: r.compression_level,
            })
            .collect())
    }

    /// Return the latest scene-level compressed summary for a session.
    pub async fn get_active_scene_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<ActiveSceneSummaryRow>, MemoryError> {
        use sea_orm::QueryOrder;

        let row = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .filter(entities::memory_spans::Column::CompressedSummary.is_not_null())
            .order_by_desc(entities::memory_spans::Column::CompressionLevel)
            .order_by_desc(entities::memory_spans::Column::TurnEnd)
            .one(&self.db)
            .await?;

        Ok(row.and_then(|r| {
            let summary = r.compressed_summary?;
            if summary.trim().is_empty() {
                return None;
            }
            Some(ActiveSceneSummaryRow {
                span_id: r.id,
                summary,
                compression_level: r.compression_level,
            })
        }))
    }

    /// Update the compressed summary for an existing span.
    pub async fn update_span_summary(
        &self,
        span_id: i64,
        summary: &str,
    ) -> Result<(), MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set};

        let mut active: entities::memory_spans::ActiveModel = entities::memory_spans::ActiveModel {
            id: Set(span_id),
            ..Default::default()
        };
        active.compressed_summary = Set(Some(summary.to_string()));
        active.update(&self.db).await?;
        Ok(())
    }

    // ── Companion Commitments ─────────────────────────────────────────────────

    /// Insert a new commitment row and return its assigned ID.
    pub async fn insert_commitment(&self, item: &crate::NewCommitment) -> Result<i64, MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let active = entities::commitments::ActiveModel {
            character_id: Set(item.character_id.clone()),
            user_id: Set(item.user_id.clone()),
            title: Set(item.title.clone()),
            description: Set(item.description.clone()),
            status: Set(item.status.as_str().to_string()),
            due_at: Set(item.due_at),
            due_label: Set(item.due_label.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
            ..Default::default()
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// Retrieve a commitment by its ID.
    pub async fn get_commitment(&self, id: i64) -> Result<Option<crate::Commitment>, MemoryError> {
        let maybe_model = entities::commitments::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        match maybe_model {
            Some(m) => model_to_commitment(m).map(Some),
            None => Ok(None),
        }
    }

    /// List active commitments for a character, optionally scoped to a user.
    ///
    /// Results are ordered by `due_at` ascending (nulls last), then `created_at`
    /// descending so undated follow-ups still surface recently.
    pub async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::Commitment>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::commitments::Entity::find()
            .filter(entities::commitments::Column::CharacterId.eq(character_id))
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            );

        if let Some(uid) = user_id {
            query = query.filter(entities::commitments::Column::UserId.eq(uid));
        }

        // SQLite sorts NULLs first on plain ASC; order by `due_at IS NULL` so dated rows
        // surface before undated follow-ups.
        let models = query
            .order_by_asc(Expr::cust("due_at IS NULL"))
            .order_by_asc(entities::commitments::Column::DueAt)
            .order_by_desc(entities::commitments::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_commitment)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Transition an active commitment to a new lifecycle status.
    ///
    /// Returns `Ok(false)` when the row does not exist or is no longer `active`.
    pub async fn update_commitment_status(
        &self,
        id: i64,
        new_status: crate::CommitmentStatus,
    ) -> Result<bool, MemoryError> {
        use sea_orm::sea_query::Expr;
        use sea_orm::{EntityTrait, QueryFilter};

        let now = Utc::now();
        let mut stmt = entities::commitments::Entity::update_many()
            .col_expr(
                entities::commitments::Column::Status,
                Expr::value(new_status.as_str().to_string()),
            )
            .col_expr(entities::commitments::Column::UpdatedAt, Expr::value(now))
            .filter(entities::commitments::Column::Id.eq(id))
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            );
        if new_status == crate::CommitmentStatus::Done {
            stmt = stmt.col_expr(
                entities::commitments::Column::CompletedAt,
                Expr::value(Some(now)),
            );
        }
        let result = stmt.exec(&self.db).await?;
        Ok(result.rows_affected > 0)
    }

    /// Update an active commitment's description and due label in-place.
    ///
    /// Only succeeds when the row exists and is `Active`. Returns `Ok(false)`
    /// when the row does not exist or is no longer active.
    ///
    /// Uses a single atomic `UPDATE ... WHERE status = 'active'` to prevent
    /// TOCTOU races between the former find→check→update round trips.
    pub async fn supersede_commitment(
        &self,
        id: i64,
        description: &str,
        due_label: Option<&str>,
    ) -> Result<bool, MemoryError> {
        use sea_orm::sea_query::Expr;
        use sea_orm::{EntityTrait, QueryFilter};

        let now = Utc::now();
        let result = entities::commitments::Entity::update_many()
            .col_expr(
                entities::commitments::Column::Description,
                Expr::value(description.to_string()),
            )
            .col_expr(
                entities::commitments::Column::DueLabel,
                Expr::value(due_label.map(ToOwned::to_owned)),
            )
            .col_expr(entities::commitments::Column::UpdatedAt, Expr::value(now))
            .filter(entities::commitments::Column::Id.eq(id))
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            )
            .exec(&self.db)
            .await?;
        Ok(result.rows_affected > 0)
    }

    /// Mark a commitment as done.
    pub async fn complete_commitment(&self, id: i64) -> Result<bool, MemoryError> {
        self.update_commitment_status(id, crate::CommitmentStatus::Done)
            .await
    }

    /// Mark a commitment as cancelled.
    pub async fn cancel_commitment(&self, id: i64) -> Result<bool, MemoryError> {
        self.update_commitment_status(id, crate::CommitmentStatus::Cancelled)
            .await
    }

    /// Mark active commitments whose `due_at` is before `now` as stale.
    ///
    /// Returns the number of rows updated.
    pub async fn mark_stale_commitments(&self, now: DateTime<Utc>) -> Result<usize, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter};

        let rows = entities::commitments::Entity::find()
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            )
            .filter(entities::commitments::Column::DueAt.is_not_null())
            .filter(entities::commitments::Column::DueAt.lt(now))
            .all(&self.db)
            .await?;

        let mut updated = 0usize;
        for model in rows {
            if self
                .update_commitment_status(model.id, crate::CommitmentStatus::Stale)
                .await?
            {
                updated = updated.saturating_add(1);
            }
        }
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_store() -> MemoryStore {
        MemoryStore::open_in_memory(4).await.unwrap()
    }

    #[test]
    fn embedding_bytes_roundtrip() {
        let original = vec![1.0_f32, 0.5, -0.25, 0.0];
        let bytes = embedding_to_bytes(&original);
        let restored = bytes_to_embedding(&bytes);
        for (a, b) in original.iter().zip(restored.iter()) {
            assert!((a - b).abs() < 1e-7, "Mismatch: {a} != {b}");
        }
    }

    fn new_session_meta(session_id: &str, card_name: &str) -> crate::session::NewSessionMeta {
        crate::session::NewSessionMeta {
            session_id: session_id.to_string(),
            card_name: card_name.to_string(),
            title: format!("{session_id} title"),
        }
    }

    #[tokio::test]
    async fn session_upsert_get_list_archive() {
        let store = setup_store().await;

        let id_a = store
            .upsert_session(&new_session_meta("sess-a", "card"))
            .await
            .unwrap();
        let id_b = store
            .upsert_session(&new_session_meta("sess-b", "card"))
            .await
            .unwrap();
        assert_ne!(id_a, id_b);

        // Upserting an existing session refreshes updated_at but keeps the row.
        let id_a2 = store
            .upsert_session(&new_session_meta("sess-a", "card"))
            .await
            .unwrap();
        assert_eq!(id_a, id_a2);

        // Touch bumps turn_count.
        store.touch_session("sess-a", 5).await.unwrap();
        let meta = store.get_session("sess-a").await.unwrap().unwrap();
        assert_eq!(meta.turn_count, 5);
        assert!(!meta.archived);

        // Listing (newest first) excludes archived by default.
        store.set_session_archived("sess-b", true).await.unwrap();
        let active = store.list_sessions(false, 10).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active.first().unwrap().session_id, "sess-a");

        let all = store.list_sessions(true, 10).await.unwrap();
        assert_eq!(all.len(), 2);

        // Archiving a missing session reports no update.
        let updated = store.set_session_archived("nope", true).await.unwrap();
        assert!(!updated);
    }

    #[tokio::test]
    async fn message_search_is_case_insensitive_and_paginated() {
        let store = setup_store().await;
        store
            .upsert_session(&new_session_meta("s1", "card"))
            .await
            .unwrap();
        store
            .upsert_session(&new_session_meta("s2", "card"))
            .await
            .unwrap();

        store
            .insert_log("s1", "card", "user", "Hello World")
            .await
            .unwrap();
        store
            .insert_log("s2", "card", "assistant", "hello there")
            .await
            .unwrap();
        store
            .insert_log("s1", "card", "user", "goodbye")
            .await
            .unwrap();

        // Case-insensitive match across sessions.
        let hits = store.search_messages("HELLO", 10, 0).await.unwrap();
        assert_eq!(hits.len(), 2);
        let sessions: std::collections::HashSet<&str> =
            hits.iter().map(|(sid, _)| sid.as_str()).collect();
        assert!(sessions.contains("s1"));
        assert!(sessions.contains("s2"));

        // Pagination.
        let page = store.search_messages("hello", 1, 0).await.unwrap();
        assert_eq!(page.len(), 1);
        let page2 = store.search_messages("hello", 1, 1).await.unwrap();
        assert_eq!(page2.len(), 1);

        // Empty query returns nothing.
        let empty = store.search_messages("", 10, 0).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn export_import_roundtrip_and_conflict() {
        let store = setup_store().await;
        store
            .upsert_session(&new_session_meta("orig", "card"))
            .await
            .unwrap();
        store
            .insert_log("orig", "card", "user", "my key is sk-secret123 bye")
            .await
            .unwrap();
        store
            .insert_log("orig", "card", "assistant", "noted")
            .await
            .unwrap();

        let export = store.build_export("orig").await.unwrap();
        assert_eq!(export.session.session_id, "orig");
        assert_eq!(export.messages.len(), 2);
        // Secrets in message content are redacted (order-independent).
        let joined: String = export.messages.iter().map(|m| m.content.as_str()).collect();
        assert!(joined.contains("[redacted]"));
        assert!(!joined.contains("sk-secret123"));

        // JSON round-trip.
        let json = export.to_json().unwrap();
        let parsed = crate::export::SessionExport::from_json(&json).unwrap();
        assert_eq!(parsed.session.session_id, "orig");

        // Importing under an existing session_id allocates a new one.
        let new_id = store.import_export(&parsed).await.unwrap();
        let sessions = store.list_sessions(true, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        let imported = sessions.iter().find(|s| s.id == new_id).unwrap();
        assert!(imported.session_id.starts_with("imported-"));
        let imported_msgs = store.list_messages(&imported.session_id).await.unwrap();
        assert_eq!(imported_msgs.len(), 2);

        // Importing a brand-new session_id preserves it.
        let mut fresh = parsed.clone();
        fresh.session.session_id = "brand-new".to_string();
        store.import_export(&fresh).await.unwrap();
        assert!(store.get_session("brand-new").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn tool_embedding_field_upsert_overwrites_and_list_filters() {
        let store = setup_store().await;
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

        store
            .upsert_tool_embedding_field(
                "web_search",
                "description",
                "",
                "hash-a",
                "",
                &emb,
                "desc text",
            )
            .await
            .unwrap();
        store
            .upsert_tool_embedding_field(
                "web_search",
                "summary",
                "",
                "hash-a",
                "",
                &emb,
                "sum text",
            )
            .await
            .unwrap();
        store
            .upsert_tool_embedding_field(
                "web_search",
                "negative",
                "",
                "hash-a",
                "",
                &emb,
                "neg text",
            )
            .await
            .unwrap();
        store
            .upsert_tool_embedding_field(
                "other_tool",
                "description",
                "",
                "hash-b",
                "",
                &emb,
                "other desc",
            )
            .await
            .unwrap();

        let rows = store.list_tool_embedding_fields().await.unwrap();
        assert_eq!(rows.len(), 4);
        let web_rows: Vec<_> = rows
            .iter()
            .filter(|(name, _, _, _, _, _, _)| name == "web_search")
            .collect();
        assert_eq!(web_rows.len(), 3);
        let fields: std::collections::HashSet<&str> = web_rows
            .iter()
            .map(|(_, f, _, _, _, _, _)| f.as_str())
            .collect();
        assert!(fields.contains("summary"));
        assert!(fields.contains("description"));
        assert!(fields.contains("negative"));

        // Upsert (replace) on the same (tool_name, field, field_key, model_name) overwrites.
        let emb2 = vec![0.0_f32, 1.0, 0.0, 0.0];
        store
            .upsert_tool_embedding_field(
                "web_search",
                "summary",
                "",
                "hash-a2",
                "",
                &emb2,
                "sum text v2",
            )
            .await
            .unwrap();
        let rows = store.list_tool_embedding_fields().await.unwrap();
        let web_summary = rows
            .iter()
            .find(|(n, f, _, _, _, _, _)| n == "web_search" && f == "summary")
            .unwrap();
        assert_eq!(web_summary.3, "hash-a2");
        assert_eq!(web_summary.5, emb2);
        assert_eq!(web_summary.6, "sum text v2");
    }

    #[tokio::test]
    async fn delete_tool_embeddings_cascades_to_fields() {
        let store = setup_store().await;
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

        for field in ["summary", "description", "negative"] {
            store
                .upsert_tool_embedding_field("web_search", field, "", "hash", "", &emb, "text")
                .await
                .unwrap();
        }
        store
            .upsert_tool_embedding_field(
                "keep_me",
                "description",
                "",
                "hash",
                "",
                &emb,
                "keep text",
            )
            .await
            .unwrap();

        assert_eq!(store.list_tool_embedding_fields().await.unwrap().len(), 4);
        let deleted = store.delete_tool_embeddings("web_search").await.unwrap();
        assert_eq!(deleted, 3);
        assert_eq!(store.list_tool_embedding_fields().await.unwrap().len(), 1);
    }

    /// Regression test for #41 (bug 4): embedding insert
    /// must reject vectors whose length does not match
    /// `embedding_dim` and vectors containing NaN /
    /// Infinity, returning a typed `InvalidEmbedding`
    /// error rather than letting the row be silently
    /// persisted and poisoning later cosine queries.
    #[tokio::test]
    async fn upsert_memory_embedding_rejects_bad_embedding() {
        let store = setup_store().await;
        let id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: crate::MemoryKind::Semantic,
                title: "fact".into(),
                content: "content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::default(),
                salience: crate::MemorySalience::default(),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();

        let wrong_len = vec![0.1, 0.2, 0.3];
        let err = store
            .upsert_memory_embedding(id, "test-model", "content", &wrong_len)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::InvalidEmbedding(_)),
            "expected InvalidEmbedding, got {err:?}"
        );

        let with_nan = vec![0.1, f32::NAN, 0.3, 0.4];
        let err = store
            .upsert_memory_embedding(id, "test-model", "content", &with_nan)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::InvalidEmbedding(_)),
            "expected InvalidEmbedding, got {err:?}"
        );

        let with_inf = vec![0.1, 0.2, f32::INFINITY, 0.4];
        let err = store
            .upsert_memory_embedding(id, "test-model", "content", &with_inf)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::InvalidEmbedding(_)),
            "expected InvalidEmbedding, got {err:?}"
        );

        let ok = vec![0.1, 0.2, 0.3, 0.4];
        store
            .upsert_memory_embedding(id, "test-model", "content", &ok)
            .await
            .unwrap();
    }

    /// Regression test for #41 (bug 1): the memory store
    /// must apply `foreign_keys=ON` (and the other
    /// safety PRAGMAs) on every connection it opens. For
    /// an in-memory store `journal_mode=WAL` is a no-op
    /// (`SQLite` returns `memory`), but `foreign_keys` and
    /// `busy_timeout` are still meaningful.
    #[tokio::test]
    async fn pragmas_are_applied_on_open() {
        use sea_orm::ConnectionTrait;
        let store = setup_store().await;
        // `execute_unprepared` returns a `ExecResult`
        // whose `rows_affected` field is populated for
        // some statements; for `PRAGMA foreign_keys` it
        // returns the current value of the pragma (0 or
        // 1) as `rows_affected`. This is a pragmatic
        // way to assert the PRAGMA took effect without
        // pulling in a full query API.
        let res = store
            .connection()
            .execute_unprepared("PRAGMA foreign_keys")
            .await
            .unwrap();
        assert_eq!(
            res.rows_affected(),
            1,
            "foreign_keys PRAGMA should report 1 (ON)"
        );
    }

    #[tokio::test]
    async fn insert_conversation_turn_stores_and_retrieves() {
        let store = setup_store().await;
        let session_id = "turn-test-session";

        let ids = store
            .insert_conversation_turn(session_id, "ene", "Hello", "Hi there!")
            .await
            .unwrap();
        let logs = store.get_logs_by_session(session_id).await.unwrap();
        assert_eq!(logs.len(), 2);
        let user_log = logs.first().expect("user log");
        let assistant_log = logs.get(1).expect("assistant log");
        assert_eq!(user_log.0, "user");
        assert_eq!(user_log.1, "Hello");
        assert_eq!(assistant_log.0, "assistant");
        assert_eq!(assistant_log.1, "Hi there!");
        let _ = ids;
    }

    #[tokio::test]
    async fn affect_state_get_upsert() {
        let store = setup_store().await;

        let result = store.get_affect_state("ene").await.unwrap();
        assert!((result.valence - 0.0).abs() < f32::EPSILON);
        assert!((result.arousal - 0.0).abs() < f32::EPSILON);
        assert!(result.discrete_emotions.is_empty());

        let mut state = crate::AffectState {
            character_id: "ene".into(),
            user_id: String::new(),
            valence: 0.5,
            arousal: -0.3,
            dominance: 0.1,
            trust: 0.4,
            affinity: 0.6,
            irritation: 0.0,
            curiosity: 0.7,
            fatigue: 0.1,
            mood_label: String::new(),
            last_expression: String::new(),
            discrete_emotions: vec![
                crate::DiscreteEmotion::new("joy", 0.8),
                crate::DiscreteEmotion::new("surprise", 0.4),
            ],
            updated_at: None,
        };
        store.upsert_affect_state(&state).await.unwrap();

        let loaded = store.get_affect_state("ene").await.unwrap();
        assert!((loaded.valence - 0.5).abs() < f32::EPSILON);
        assert!((loaded.arousal + 0.3).abs() < f32::EPSILON);
        assert_eq!(loaded.discrete_emotions.len(), 2);
        let joy = loaded.discrete_emotions.first().expect("joy emotion");
        assert_eq!(joy.label, "joy");
        assert!((joy.intensity - 0.8).abs() < f32::EPSILON);

        state.valence = -0.2;
        state.discrete_emotions = vec![crate::DiscreteEmotion::new("sadness", 0.6)];
        store.upsert_affect_state(&state).await.unwrap();

        let loaded2 = store.get_affect_state("ene").await.unwrap();
        assert!((loaded2.valence + 0.2).abs() < f32::EPSILON);
        assert_eq!(loaded2.discrete_emotions.len(), 1);
        let sadness = loaded2.discrete_emotions.first().expect("sadness emotion");
        assert_eq!(sadness.label, "sadness");
    }

    #[tokio::test]
    async fn typed_memory_crud() {
        let store = setup_store().await;

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Episodic,
            title: "Greeting".into(),
            content: "The user greeted me this morning".into(),
            source: crate::MemorySource::Conversation,
            source_ref: Some("sess-1/turn-1".into()),
            confidence: crate::MemoryConfidence::new(0.9),
            salience: crate::MemorySalience::new(0.5),
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

        let id = store.insert_typed_memory(&item).await.unwrap();
        assert!(id > 0);

        let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "Greeting");
        assert_eq!(loaded.kind, crate::MemoryKind::Episodic);
        assert!((loaded.confidence.get() - 0.9).abs() < f32::EPSILON);

        let by_char = store
            .get_typed_memories_by_character("ene", None, 10, 0)
            .await
            .unwrap();
        assert!(!by_char.is_empty());

        let count = store
            .count_typed_memories("ene", Some(crate::MemoryKind::Episodic))
            .await
            .unwrap();
        assert!(count > 0);

        let status_ok = store
            .set_memory_status(id, crate::MemoryStatus::Faded)
            .await
            .unwrap();
        assert!(status_ok);

        let access_ok = store.bump_typed_memory_access(id).await.unwrap();
        assert!(access_ok);

        let loaded2 = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(loaded2.status, crate::MemoryStatus::Faded);
        assert_eq!(loaded2.access_count, 1);

        assert!(store.get_typed_memory(999_999).await.unwrap().is_none());
        assert!(
            !store
                .set_memory_status(999_999, crate::MemoryStatus::Active)
                .await
                .unwrap()
        );
        assert!(!store.bump_typed_memory_access(999_999).await.unwrap());
    }

    #[tokio::test]
    async fn typed_memory_search_with_embedding() {
        let store = setup_store().await;

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "Test memory".into(),
            content: "The user likes pizza".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.8),
            salience: crate::MemorySalience::new(0.6),
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

        let id = store.insert_typed_memory(&item).await.unwrap();
        let emb = vec![0.1, 0.2, 0.3, 0.4];
        store
            .upsert_memory_embedding(id, "test-model", "content", &emb)
            .await
            .unwrap();

        let results = store
            .search_typed_memories(&emb, "ene", "test-model", 10, 0.0)
            .await
            .unwrap();
        assert!(!results.is_empty());
        let top = results.first().expect("typed memory search result");
        assert!((top.1 - 1.0).abs() < f32::EPSILON);
        assert_eq!(top.0.title, "Test memory");
    }

    #[tokio::test]
    async fn supersede_typed_memory_links_rows() {
        let store = setup_store().await;

        let old_item = crate::NewMemoryItem {
            scope: crate::MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Preference,
            title: "drink".into(),
            content: "likes coffee".into(),
            source: crate::MemorySource::Inferred,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.7),
            salience: crate::MemorySalience::default(),
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
        let old_id = store.insert_typed_memory(&old_item).await.unwrap();

        let new_item = crate::NewMemoryItem {
            content: "likes tea".into(),
            confidence: crate::MemoryConfidence::new(0.9),
            ..old_item
        };
        let new_id = store
            .supersede_typed_memory(&new_item, old_id)
            .await
            .unwrap();

        let old = store.get_typed_memory(old_id).await.unwrap().unwrap();
        assert_eq!(old.status, crate::MemoryStatus::Superseded);
        assert_eq!(old.supersedes_id, None);

        let new_mem = store.get_typed_memory(new_id).await.unwrap().unwrap();
        assert_eq!(new_mem.supersedes_id, Some(old_id));
        assert_eq!(new_mem.status, crate::MemoryStatus::Active);
    }

    #[tokio::test]
    async fn supersede_typed_memory_rejects_terminal_status() {
        let store = setup_store().await;

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Preference,
            title: "drink".into(),
            content: "likes coffee".into(),
            source: crate::MemorySource::Inferred,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.7),
            salience: crate::MemorySalience::default(),
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
        let old_id = store.insert_typed_memory(&item).await.unwrap();
        store
            .set_memory_status(old_id, crate::MemoryStatus::UserDeleted)
            .await
            .unwrap();

        let replacement = crate::NewMemoryItem {
            content: "likes tea".into(),
            ..item
        };
        let err = store
            .supersede_typed_memory(&replacement, old_id)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::Other(_)),
            "expected Other error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn commitment_crud_and_lifecycle() {
        let store = setup_store().await;

        let id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "design review".into(),
                description: "Discuss the design next session".into(),
                status: crate::CommitmentStatus::Active,
                due_at: None,
                due_label: Some("next session".into()),
            })
            .await
            .unwrap();

        let loaded = store.get_commitment(id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "design review");

        let active = store
            .list_active_commitments("ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);

        assert!(store.complete_commitment(id).await.unwrap());
        let done = store.get_commitment(id).await.unwrap().unwrap();
        assert_eq!(done.status, crate::CommitmentStatus::Done);
        assert!(done.completed_at.is_some());

        let active_after = store
            .list_active_commitments("ene", None, 10)
            .await
            .unwrap();
        assert!(active_after.is_empty());
    }

    #[tokio::test]
    async fn mark_stale_commitments_past_due() {
        let store = setup_store().await;
        let past = Utc::now() - chrono::Duration::days(1);
        let future = Utc::now() + chrono::Duration::days(1);

        let stale_target = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: String::new(),
                title: "overdue".into(),
                description: "was due yesterday".into(),
                status: crate::CommitmentStatus::Active,
                due_at: Some(past),
                due_label: None,
            })
            .await
            .unwrap();
        let _still_active = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: String::new(),
                title: "upcoming".into(),
                description: "due tomorrow".into(),
                status: crate::CommitmentStatus::Active,
                due_at: Some(future),
                due_label: None,
            })
            .await
            .unwrap();

        let updated = store.mark_stale_commitments(Utc::now()).await.unwrap();
        assert_eq!(updated, 1);

        let stale = store.get_commitment(stale_target).await.unwrap().unwrap();
        assert_eq!(stale.status, crate::CommitmentStatus::Stale);
    }

    #[tokio::test]
    async fn list_active_commitments_orders_dated_before_undated() {
        let store = setup_store().await;
        let now = Utc::now();

        let later_id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "later".into(),
                description: "due in two days".into(),
                status: crate::CommitmentStatus::Active,
                due_at: Some(now + chrono::Duration::days(2)),
                due_label: None,
            })
            .await
            .unwrap();
        let sooner_id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "sooner".into(),
                description: "due tomorrow".into(),
                status: crate::CommitmentStatus::Active,
                due_at: Some(now + chrono::Duration::days(1)),
                due_label: None,
            })
            .await
            .unwrap();
        let undated_id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "undated".into(),
                description: "no due date".into(),
                status: crate::CommitmentStatus::Active,
                due_at: None,
                due_label: Some("next time".into()),
            })
            .await
            .unwrap();

        let active = store
            .list_active_commitments("ene", Some("user1"), 10)
            .await
            .unwrap();
        let ids: Vec<i64> = active.iter().map(|c| c.id.unwrap()).collect();
        assert_eq!(ids, vec![sooner_id, later_id, undated_id]);
    }

    #[tokio::test]
    async fn terminal_commitment_status_is_not_overwritten() {
        let store = setup_store().await;
        let past = Utc::now() - chrono::Duration::days(1);

        let done_id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "completed".into(),
                description: "already done".into(),
                status: crate::CommitmentStatus::Active,
                due_at: Some(past),
                due_label: None,
            })
            .await
            .unwrap();
        assert!(store.complete_commitment(done_id).await.unwrap());

        let updated = store.mark_stale_commitments(Utc::now()).await.unwrap();
        assert_eq!(updated, 0);

        let done = store.get_commitment(done_id).await.unwrap().unwrap();
        assert_eq!(done.status, crate::CommitmentStatus::Done);
        assert!(done.completed_at.is_some());

        assert!(!store.cancel_commitment(done_id).await.unwrap());
        assert!(
            !store
                .update_commitment_status(done_id, crate::CommitmentStatus::Stale)
                .await
                .unwrap()
        );
    }

    fn hybrid_search_options<'a>(
        query_text: &'a str,
        query_embedding: &'a [f32],
        now: DateTime<Utc>,
    ) -> crate::Query<'a> {
        crate::Query {
            query_text,
            embedding: Some(query_embedding),
            character_id: "ene",
            user_id: None,
            model_name: "test-model",
            limit: 10,
            similarity_threshold: 0.0,
            candidate_pool_size: 50,
            query_affect: None,
            weights: crate::HybridSearchWeights::default(),
            decay_half_life_days: 30.0,
            now,
            min_score: 0.0,
            commitment_boost: 0.25,
            recent_fallback_limit: 5,
        }
    }

    async fn insert_memory_with_embedding(
        store: &MemoryStore,
        item: &crate::NewMemoryItem,
        embedding: &[f32],
    ) -> i64 {
        let id = store.insert_typed_memory(item).await.unwrap();
        store
            .upsert_memory_embedding(id, "test-model", "content", embedding)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn hybrid_search_ranks_by_salience_and_recency_not_vector_alone() {
        let store = setup_store().await;
        let now = Utc::now();
        let query_emb = vec![1.0, 0.0, 0.0, 0.0];

        let low_salience = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "distant topic".into(),
            content: "unrelated content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.5),
            salience: crate::MemorySalience::new(0.2),
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
        let high_salience = crate::NewMemoryItem {
            salience: crate::MemorySalience::new(0.95),
            confidence: crate::MemoryConfidence::new(0.9),
            title: "important fact".into(),
            content: "user preference about music".into(),
            ..low_salience.clone()
        };

        insert_memory_with_embedding(&store, &low_salience, &query_emb).await;
        insert_memory_with_embedding(&store, &high_salience, &query_emb).await;

        let options = hybrid_search_options("music preference", &query_emb, now);
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 2);
        let top = results.first().expect("top hybrid result");
        let second = results.get(1).expect("second hybrid result");
        assert_eq!(top.item.title, "important fact");
        assert!(top.breakdown.total >= second.breakdown.total);
    }

    #[tokio::test]
    async fn hybrid_search_lexical_component_for_matching_query() {
        let store = setup_store().await;
        let now = Utc::now();
        let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "favorite drink".into(),
            content: "The user loves matcha latte".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
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
        insert_memory_with_embedding(&store, &item, &orthogonal).await;

        let options = hybrid_search_options("matcha latte", &orthogonal, now);
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        let result = results.first().expect("lexical match result");
        assert!(result.breakdown.lexical_score > 0.0);
        assert!(
            result
                .sources
                .contains(&crate::MemoryCandidateSource::Lexical)
        );
    }

    #[tokio::test]
    async fn hybrid_search_surfaces_active_commitment_with_low_vector_similarity() {
        let store = setup_store().await;
        let now = Utc::now();
        let query_emb = vec![1.0, 0.0, 0.0, 0.0];

        let commitment_id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "follow up".into(),
                description: "Review the architecture document".into(),
                status: crate::CommitmentStatus::Active,
                due_at: None,
                due_label: Some("next time".into()),
            })
            .await
            .unwrap();

        let options = hybrid_search_options("unrelated query", &query_emb, now);
        let results = store.search(&options).await.unwrap();
        assert!(
            results.iter().any(|r| {
                r.item.commitment_id == Some(commitment_id)
                    && r.breakdown.commitment_boost > 0.0
                    && r.sources
                        .contains(&crate::MemoryCandidateSource::Commitment)
            }),
            "active ledger commitment should be recalled despite low vector similarity"
        );
    }

    #[tokio::test]
    async fn hybrid_search_excludes_archived_superseded_and_user_deleted() {
        let store = setup_store().await;
        let now = Utc::now();
        let emb = vec![0.5, 0.5, 0.5, 0.5];

        let base = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "memory".into(),
            content: "shared content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
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

        for status in [
            crate::MemoryStatus::Archived,
            crate::MemoryStatus::Superseded,
            crate::MemoryStatus::UserDeleted,
        ] {
            let mut item = base.clone();
            item.title = format!("{status:?}");
            item.status = status;
            insert_memory_with_embedding(&store, &item, &emb).await;
        }

        let options = hybrid_search_options("shared content", &emb, now);
        let results = store.search(&options).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_search_faded_memory_has_stale_penalty() {
        let store = setup_store().await;
        let now = Utc::now();
        let emb = vec![1.0, 0.0, 0.0, 0.0];

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "old fact".into(),
            content: "faded memory content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Faded,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        insert_memory_with_embedding(&store, &item, &emb).await;

        let options = hybrid_search_options("faded memory", &emb, now);
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        let result = results.first().expect("faded memory result");
        assert!(result.breakdown.vector_similarity > 0.0);
        assert!(result.breakdown.stale_penalty > 0.0);
    }

    #[tokio::test]
    async fn hybrid_search_finds_old_lexical_match_outside_recent_pool() {
        let store = setup_store().await;
        let now = Utc::now();
        let query_emb = vec![0.0, 0.0, 1.0, 0.0];
        let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

        let old_item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "ancient dragon recipe".into(),
            content: "A very old note about ancient dragon recipe".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
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
        let old_id = store.insert_typed_memory(&old_item).await.unwrap();
        store
            .upsert_memory_embedding(old_id, "test-model", "content", &orthogonal)
            .await
            .unwrap();

        for i in 0..10 {
            let filler = crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: crate::MemoryKind::Semantic,
                title: format!("recent filler {i}"),
                content: "unrelated filler content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::default(),
                salience: crate::MemorySalience::new(0.95),
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
            insert_memory_with_embedding(&store, &filler, &orthogonal).await;
        }

        let mut options = hybrid_search_options("ancient dragon recipe", &query_emb, now);
        options.recent_fallback_limit = 0;
        options.similarity_threshold = 0.8;
        let results = store.search(&options).await.unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.item.id == Some(old_id) && r.breakdown.lexical_score > 0.0),
            "old lexical match should be found outside the recent pool"
        );
    }

    #[tokio::test]
    async fn hybrid_search_excludes_unrelated_recent_without_fallback() {
        let store = setup_store().await;
        let now = Utc::now();
        let query_emb = vec![1.0, 0.0, 0.0, 0.0];
        let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

        let unrelated = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "fresh but unrelated".into(),
            content: "nothing to do with the query".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::new(0.99),
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
        insert_memory_with_embedding(&store, &unrelated, &orthogonal).await;

        let mut options = hybrid_search_options("completely different topic", &query_emb, now);
        options.recent_fallback_limit = 0;
        options.similarity_threshold = 0.8;
        let results = store.search(&options).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_search_ranks_higher_confidence_when_other_signals_match() {
        let store = setup_store().await;
        let now = Utc::now();
        let query_emb = vec![1.0, 0.0, 0.0, 0.0];

        let base = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "shared topic".into(),
            content: "shared topic content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.2),
            salience: crate::MemorySalience::new(0.5),
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
        let low_confidence = base.clone();
        let high_confidence = crate::NewMemoryItem {
            title: "shared topic high".into(),
            confidence: crate::MemoryConfidence::new(0.95),
            ..base
        };

        insert_memory_with_embedding(&store, &low_confidence, &query_emb).await;
        insert_memory_with_embedding(&store, &high_confidence, &query_emb).await;

        let options = hybrid_search_options("shared topic", &query_emb, now);
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 2);
        let top = results.first().expect("top confidence result");
        let second = results.get(1).expect("second confidence result");
        assert_eq!(top.item.title, "shared topic high");
        assert!(top.breakdown.confidence > second.breakdown.confidence);
    }

    #[tokio::test]
    async fn hybrid_search_respects_user_id_scope() {
        let store = setup_store().await;
        let now = Utc::now();
        let query_emb = vec![1.0, 0.0, 0.0, 0.0];

        let base = crate::NewMemoryItem {
            scope: crate::MemoryScope::User,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "scoped memory".into(),
            content: "user scoped content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
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

        let mut user1_item = base.clone();
        user1_item.user_id = "user1".into();
        let mut user2_item = base;
        user2_item.user_id = "user2".into();

        insert_memory_with_embedding(&store, &user1_item, &query_emb).await;
        insert_memory_with_embedding(&store, &user2_item, &query_emb).await;

        let mut options = hybrid_search_options("scoped memory", &query_emb, now);
        options.user_id = Some("user1");
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        let result = results.first().expect("scoped search result");
        assert_eq!(result.item.user_id, "user1");
    }

    #[tokio::test]
    async fn hybrid_search_dedupes_multi_source_candidates() {
        let store = setup_store().await;
        let now = Utc::now();
        let emb = vec![1.0, 0.0, 0.0, 0.0];

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "pizza night".into(),
            content: "Friday pizza tradition".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
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
        insert_memory_with_embedding(&store, &item, &emb).await;

        let options = hybrid_search_options("pizza tradition", &emb, now);
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        let result = results.first().expect("deduped search result");
        assert!(result.sources.len() >= 2);
    }

    #[tokio::test]
    async fn set_memory_status_rejects_invalid_edge() {
        let store = setup_store().await;
        let id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: crate::MemoryKind::Semantic,
                title: "fact".into(),
                content: "content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::default(),
                salience: crate::MemorySalience::default(),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Faded,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();

        let err = store
            .set_memory_status(id, crate::MemoryStatus::Active)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MemoryError::InvalidTransition {
                from: crate::MemoryStatus::Faded,
                to: crate::MemoryStatus::Active,
            }
        ));
    }

    #[tokio::test]
    async fn apply_natural_decay_batch_fades_and_archives() {
        let store = setup_store().await;
        let now = Utc::now();

        let active_id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Semantic,
                title: "old active".into(),
                content: "old content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.2),
                salience: crate::MemorySalience::new(0.1),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store
            .test_backdate_typed_memory(active_id, 120)
            .await
            .unwrap();

        let faded_id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Semantic,
                title: "old faded".into(),
                content: "very old content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.1),
                salience: crate::MemorySalience::new(0.1),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Faded,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store
            .test_backdate_typed_memory(faded_id, 365)
            .await
            .unwrap();

        let report = store
            .apply_natural_decay_batch("ene", Some("user1"), now, 30.0, 64)
            .await
            .unwrap();
        assert!(report.faded_count >= 1);
        assert!(report.archived_count >= 1);

        let active_loaded = store.get_typed_memory(active_id).await.unwrap().unwrap();
        assert_eq!(active_loaded.status, crate::MemoryStatus::Faded);

        let faded_loaded = store.get_typed_memory(faded_id).await.unwrap().unwrap();
        assert_eq!(faded_loaded.status, crate::MemoryStatus::Archived);
    }

    #[tokio::test]
    async fn pin_typed_memory_excludes_from_natural_decay() {
        let store = setup_store().await;
        let id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: crate::MemoryKind::Semantic,
                title: "pinned".into(),
                content: "pinned content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.1),
                salience: crate::MemorySalience::new(0.1),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: true,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store.test_backdate_typed_memory(id, 200).await.unwrap();

        let report = store
            .apply_natural_decay_batch("ene", None, Utc::now(), 30.0, 64)
            .await
            .unwrap();
        assert_eq!(report.faded_count, 0);

        let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(loaded.status, crate::MemoryStatus::Active);
        assert!(loaded.pinned);
    }

    #[tokio::test]
    async fn transition_active_to_faded_sets_faded_at_from_decay_anchor() {
        let store = setup_store().await;
        let id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Semantic,
                title: "anchor test".into(),
                content: "anchor content".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::default(),
                salience: crate::MemorySalience::default(),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store.test_backdate_typed_memory(id, 45).await.unwrap();

        let before = store.get_typed_memory(id).await.unwrap().unwrap();
        assert!(
            store
                .set_memory_status(id, crate::MemoryStatus::Faded)
                .await
                .unwrap()
        );

        let after = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(after.status, crate::MemoryStatus::Faded);
        assert_eq!(after.faded_at, Some(before.updated_at));
    }

    #[tokio::test]
    async fn single_row_natural_decay_reaches_archived_in_two_passes() {
        let store = setup_store().await;
        let now = Utc::now();

        let id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Semantic,
                title: "ancient".into(),
                content: "very old fact".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.1),
                salience: crate::MemorySalience::new(0.1),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store.test_backdate_typed_memory(id, 365).await.unwrap();

        let first = store
            .apply_natural_decay_batch("ene", Some("user1"), now, 30.0, 64)
            .await
            .unwrap();
        assert_eq!(first.faded_count, 1);
        assert_eq!(first.archived_count, 0);

        let faded = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(faded.status, crate::MemoryStatus::Faded);
        assert!(faded.faded_at.is_some());

        let second = store
            .apply_natural_decay_batch("ene", Some("user1"), now, 30.0, 64)
            .await
            .unwrap();
        assert_eq!(second.archived_count, 1);

        let archived = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(archived.status, crate::MemoryStatus::Archived);
    }

    #[tokio::test]
    async fn hybrid_search_preserves_pinned_flag() {
        let store = setup_store().await;
        let now = Utc::now();
        let emb = vec![1.0, 0.0, 0.0, 0.0];

        let item = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "pinned fact".into(),
            content: "pinned vector content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: true,
            created_at: None,
            commitment_id: None,
        };
        insert_memory_with_embedding(&store, &item, &emb).await;

        let options = hybrid_search_options("pinned vector", &emb, now);
        let results = store.search(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        let result = results.first().expect("pinned search result");
        assert!(result.item.pinned);
    }
}
