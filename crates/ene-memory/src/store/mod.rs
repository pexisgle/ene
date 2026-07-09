use crate::entities;
use crate::error::MemoryError;
use crate::migrator::Migrator;
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    FromQueryResult, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};

use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm_migration::MigratorTrait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Legacy table write policy for cognitive runtime integration (#98).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegacyWriteMode {
    /// Legacy summaries/keyfacts/logs may be written (default).
    #[default]
    ReadWrite = 0,
    /// Legacy writes rejected; tables remain for read-only recall.
    ReadOnly = 1,
}

impl LegacyWriteMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::ReadOnly,
            _ => Self::ReadWrite,
        }
    }
}

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
    ONCE.call_once(|| unsafe {
        // SAFETY: sqlite3_auto_extension expects a function pointer cast to a void pointer.
        // sqlite3_vec_init is a C function with the correct signature (extern "C" fn()),
        // and transmuting it to *const () is a well-known pattern for registering SQLite
        // extensions. The function pointer remains valid for the lifetime of the process.
        sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut libsqlite3_sys::sqlite3,
                *mut *mut i8,
                *const libsqlite3_sys::sqlite3_api_routines,
            ) -> i32,
        >(sqlite3_vec_init as *const ())));
    });
}

fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
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

/// A key-value fact about the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFact {
    /// The key identifier for this fact.
    pub key: String,
    /// The value associated with the key.
    pub value: String,
}

/// A stored conversation summary entry with its embedding vector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    /// Primary key.
    pub id: i64,
    /// Session identifier this summary belongs to.
    pub session_id: String,
    /// Character card name.
    pub card_name: String,
    /// The summary text.
    pub summary: String,
    /// Vector embedding of the summary.
    pub embedding: Vec<f32>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Timestamp when the session ended.
    pub ended_at: DateTime<Utc>,
}

/// A recalled summary with its cosine similarity score.
#[derive(Debug, Clone)]
pub struct RecalledSummary {
    /// The recalled conversation summary entry.
    pub entry: ConversationSummary,
    /// Cosine similarity score.
    pub similarity: f32,
}

/// SQLite-backed long-term memory store with vector similarity search.
///
/// Uses `SeaORM` for async database connection management and `sqlite-vec` for cosine-similarity queries.
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
    legacy_write_mode: AtomicU8,
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
    })
}

/// Convert a commitment model row to a [`crate::Commitment`].
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
        source_memory_id: m.source_memory_id,
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

fn is_supersedeable_status(status: crate::MemoryStatus) -> bool {
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
    let id = match item.id {
        Some(id) => id,
        None => return,
    };
    gathered
        .entry(id)
        .and_modify(|candidate| {
            candidate.vector_similarity = candidate.vector_similarity.max(vector_similarity);
            if !candidate.sources.contains(&source) {
                candidate.sources.push(source);
            }
        })
        .or_insert(GatheredCandidate {
            item,
            vector_similarity,
            sources: vec![source],
        });
}

/// Applies the SQLite PRAGMAs the store depends on to the
/// given connection. Idempotent and safe to call from both
/// `open` and `open_in_memory`.
///
/// * `journal_mode=WAL` lets readers proceed concurrently
///   with a writer. WAL is a no-op for in-memory databases
///   (SQLite returns `memory`), so it is safe to issue
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

async fn typed_memory_exists_by_source_ref_on<C: ConnectionTrait>(
    conn: &C,
    source_ref: &str,
) -> Result<bool, MemoryError> {
    use sea_orm::PaginatorTrait;

    let count = entities::typed_memories::Entity::find()
        .filter(entities::typed_memories::Column::SourceRef.eq(source_ref))
        .count(conn)
        .await?;
    Ok(count > 0)
}

async fn insert_typed_memory_on<C: ConnectionTrait>(
    conn: &C,
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
        ..Default::default()
    };
    let res = active.insert(conn).await?;
    Ok(res.id)
}

async fn patch_typed_memory_created_at_on<C: ConnectionTrait>(
    conn: &C,
    memory_id: i64,
    created_at: DateTime<Utc>,
) -> Result<(), MemoryError> {
    use sea_orm::ActiveModelTrait;
    use sea_orm::ActiveValue::Set;

    let mut active: entities::typed_memories::ActiveModel =
        entities::typed_memories::Entity::find_by_id(memory_id)
            .one(conn)
            .await?
            .ok_or_else(|| MemoryError::Other(format!("memory {memory_id} not found")))?
            .into();
    active.created_at = Set(created_at);
    active.update(conn).await?;
    Ok(())
}

async fn upsert_memory_embedding_on<C: ConnectionTrait>(
    conn: &C,
    embedding_dim: usize,
    memory_item_id: i64,
    model_name: &str,
    field: &str,
    embedding: &[f32],
) -> Result<(), MemoryError> {
    use sea_orm::ActiveValue::Set;
    use sea_orm::EntityTrait;

    validate_embedding(embedding, embedding_dim)?;

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
        .exec(conn)
        .await?;

    Ok(())
}

async fn memory_span_exists_on<C: ConnectionTrait>(
    conn: &C,
    session_id: &str,
    turn_start: i32,
) -> Result<bool, MemoryError> {
    use sea_orm::PaginatorTrait;

    let count = entities::memory_spans::Entity::find()
        .filter(entities::memory_spans::Column::SessionId.eq(session_id))
        .filter(entities::memory_spans::Column::TurnStart.eq(turn_start))
        .count(conn)
        .await?;
    Ok(count > 0)
}

async fn insert_memory_span_on<C: ConnectionTrait>(
    conn: &C,
    span: &NewMemorySpan,
) -> Result<i64, MemoryError> {
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
    let res = active.insert(conn).await?;
    Ok(res.id)
}

async fn mark_migration_complete_on<C: ConnectionTrait>(
    conn: &C,
    card_name: &str,
    counts: crate::LegacyRowCounts,
    strategy: &str,
) -> Result<(), MemoryError> {
    use sea_orm::ActiveModelTrait;
    use sea_orm::ActiveValue::Set;

    let now = Utc::now();
    let active = entities::memory_migration_meta::ActiveModel {
        card_name: Set(card_name.to_string()),
        migrated_at: Set(now),
        legacy_summaries_count: Set(counts.summaries as i32),
        legacy_keyfacts_count: Set(counts.keyfacts as i32),
        legacy_logs_count: Set(counts.logs as i32),
        strategy: Set(strategy.to_string()),
    };
    active.insert(conn).await?;
    Ok(())
}

impl MemoryStore {
    fn init(db: DatabaseConnection, embedding_dim: usize) -> Self {
        Self {
            db,
            embedding_dim,
            legacy_write_mode: AtomicU8::new(LegacyWriteMode::ReadWrite as u8),
        }
    }

    /// Returns the current legacy write mode (#98).
    #[must_use]
    pub fn legacy_write_mode(&self) -> LegacyWriteMode {
        LegacyWriteMode::from_u8(self.legacy_write_mode.load(Ordering::Relaxed))
    }

    /// Set legacy table write mode (read-only when cognition path is active).
    pub fn set_legacy_write_mode(&self, mode: LegacyWriteMode) {
        self.legacy_write_mode.store(mode as u8, Ordering::Relaxed);
    }

    fn ensure_legacy_writable(&self) -> Result<(), MemoryError> {
        if self.legacy_write_mode() == LegacyWriteMode::ReadOnly {
            return Err(MemoryError::LegacyWriteForbidden);
        }
        Ok(())
    }

    /// Decode stored embedding bytes (used by legacy migration).
    #[must_use]
    pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32> {
        bytes_to_embedding(bytes)
    }

    /// Returns the database connection handle.
    #[must_use]
    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Returns the dimensionality of the embedding vectors.
    #[must_use]
    pub fn embedding_dim(&self) -> usize {
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
        let opt = ConnectOptions::new(format!("sqlite:{}?mode=rwc", path_str));
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

    // ── Conversation Summaries ────────────────────────────────────────────────

    /// Inserts a conversation summary and associated key facts in a single transaction.
    ///
    /// Facts with an empty `value` field are treated as deletions for that key.
    /// Returns the new summary's auto-increment ID.
    pub async fn insert_summary(
        &self,
        session_id: &str,
        card_name: &str,
        summary: &str,
        key_facts: &[KeyFact],
        embedding: &[f32],
        ended_at: DateTime<Utc>,
    ) -> Result<i64, MemoryError> {
        self.ensure_legacy_writable()?;
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        validate_embedding(embedding, self.embedding_dim)?;

        let now = Utc::now();

        let session_id = session_id.to_string();
        let card_name = card_name.to_string();
        let summary = summary.to_string();
        let key_facts = key_facts.to_vec();
        let embedding_bytes = embedding_to_bytes(embedding);

        let summary_id = self
            .db
            .transaction::<_, i64, MemoryError>(|txn| {
                Box::pin(async move {
                    let new_summary = entities::conversation_summaries::ActiveModel {
                        session_id: Set(session_id),
                        card_name: Set(card_name.clone()),
                        summary: Set(summary),
                        embedding: Set(embedding_bytes),
                        created_at: Set(now),
                        ended_at: Set(ended_at),
                        ..Default::default()
                    };

                    let res = new_summary.insert(txn).await?;
                    let summary_id = res.id;

                    for fact in key_facts {
                        if fact.value.is_empty() {
                            entities::conversation_keyfacts::Entity::delete_many()
                                .filter(
                                    entities::conversation_keyfacts::Column::CardName
                                        .eq(&card_name),
                                )
                                .filter(entities::conversation_keyfacts::Column::Key.eq(&fact.key))
                                .exec(txn)
                                .await?;
                        } else {
                            let new_fact = entities::conversation_keyfacts::ActiveModel {
                                card_name: Set(card_name.clone()),
                                summary_id: Set(Some(summary_id)),
                                key: Set(fact.key),
                                value: Set(fact.value),
                                created_at: Set(now),
                                ..Default::default()
                            };
                            new_fact.insert(txn).await?;
                        }
                    }

                    Ok(summary_id)
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => {
                    MemoryError::MemoryStoreError(db_err)
                }
                sea_orm::TransactionError::Transaction(e) => e,
            })?;

        Ok(summary_id)
    }

    /// Searches summaries by cosine similarity to the query embedding.
    ///
    /// Uses `vec_distance_cosine` for fast approximate matching.
    /// Results are filtered by `card_name` and `similarity_threshold`.
    pub async fn search_summaries(
        &self,
        query_embedding: &[f32],
        card_name: &str,
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<RecalledSummary>, MemoryError> {
        validate_embedding(query_embedding, self.embedding_dim)?;

        #[derive(Debug, FromQueryResult)]
        struct SearchSummaryResultRow {
            id: i64,
            session_id: String,
            card_name: String,
            summary: String,
            embedding: Vec<u8>,
            created_at: DateTime<Utc>,
            ended_at: DateTime<Utc>,
            similarity: f64,
        }

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = cosine_similarity_expr("embedding", &query_bytes);

        // TODO: refactor the threshold filter to reference
        // the projected `similarity` column once the
        // SeaORM `expr_as` / `Expr::col` API supports an
        // `IdenStatic` alias. Today, sea-orm's
        // `SimpleExpr` lacks a `gte` method, so the
        // filter has to re-evaluate the expression.
        let select = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .expr_as(similarity_expr, "similarity")
            .filter(cosine_similarity_filter(
                "embedding",
                &query_bytes,
                f64::from(similarity_threshold),
            ))
            .order_by_desc(Expr::col("similarity"))
            .limit(limit as u64);

        let results = select
            .into_model::<SearchSummaryResultRow>()
            .all(&self.db)
            .await?;

        results
            .into_iter()
            .map(|row| {
                Ok(RecalledSummary {
                    entry: ConversationSummary {
                        id: row.id,
                        session_id: row.session_id,
                        card_name: row.card_name,
                        summary: row.summary,
                        embedding: bytes_to_embedding(&row.embedding),
                        created_at: row.created_at,
                        ended_at: row.ended_at,
                    },
                    similarity: row.similarity as f32,
                })
            })
            .collect()
    }

    /// Lists the most recent conversation summaries for a card.
    pub async fn list_recent_summaries(
        &self,
        card_name: &str,
        limit: usize,
    ) -> Result<Vec<ConversationSummary>, MemoryError> {
        let rows = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .order_by_desc(entities::conversation_summaries::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        rows.into_iter()
            .map(|row| {
                Ok(ConversationSummary {
                    id: row.id,
                    session_id: row.session_id,
                    card_name: row.card_name,
                    summary: row.summary,
                    embedding: bytes_to_embedding(&row.embedding),
                    created_at: row.created_at,
                    ended_at: row.ended_at,
                })
            })
            .collect()
    }

    /// Counts the number of summaries for a card.
    pub async fn count_summaries(&self, card_name: &str) -> Result<i64, MemoryError> {
        let count = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .count(&self.db)
            .await?;
        Ok(count as i64)
    }

    /// Deletes a summary and its associated keyfacts.
    pub async fn delete_summary(&self, id: i64) -> Result<usize, MemoryError> {
        let db = &self.db;
        db.transaction::<_, usize, MemoryError>(|txn| {
            Box::pin(async move {
                entities::conversation_keyfacts::Entity::delete_many()
                    .filter(entities::conversation_keyfacts::Column::SummaryId.eq(id))
                    .exec(txn)
                    .await?;

                let res = entities::conversation_summaries::Entity::delete_by_id(id)
                    .exec(txn)
                    .await?;

                Ok(res.rows_affected as usize)
            })
        })
        .await
        .map_err(|e| match e {
            sea_orm::TransactionError::Connection(db_err) => MemoryError::MemoryStoreError(db_err),
            sea_orm::TransactionError::Transaction(e) => e,
        })
    }

    // ── Key Facts ─────────────────────────────────────────────────────────────

    /// Returns all unique keyfacts for a card, with the latest value per key.
    pub async fn get_all_keyfacts(&self, card_name: &str) -> Result<Vec<KeyFact>, MemoryError> {
        let rows = entities::conversation_keyfacts::Entity::find()
            .filter(entities::conversation_keyfacts::Column::CardName.eq(card_name))
            .order_by_asc(entities::conversation_keyfacts::Column::Key)
            .order_by_desc(entities::conversation_keyfacts::Column::CreatedAt)
            .all(&self.db)
            .await?;

        let mut seen_keys: std::collections::HashSet<String> = Default::default();
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                if seen_keys.insert(row.key.clone()) {
                    Some(KeyFact {
                        key: row.key,
                        value: row.value,
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    /// Inserts or updates a keyfact for a card.
    pub async fn upsert_keyfact(
        &self,
        card_name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), MemoryError> {
        self.ensure_legacy_writable()?;
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let card_name = card_name.to_string();
        let key = key.to_string();
        let value = value.to_string();

        self.db
            .transaction::<_, (), MemoryError>(|txn| {
                let card_name = card_name.clone();
                let key = key.clone();
                let value = value.clone();
                Box::pin(async move {
                    entities::conversation_keyfacts::Entity::delete_many()
                        .filter(entities::conversation_keyfacts::Column::CardName.eq(&card_name))
                        .filter(entities::conversation_keyfacts::Column::Key.eq(&key))
                        .exec(txn)
                        .await?;

                    let new_fact = entities::conversation_keyfacts::ActiveModel {
                        card_name: Set(card_name),
                        summary_id: Set(Some(0)),
                        key: Set(key),
                        value: Set(value),
                        created_at: Set(now),
                        ..Default::default()
                    };
                    new_fact.insert(txn).await?;

                    Ok(())
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => {
                    MemoryError::MemoryStoreError(db_err)
                }
                sea_orm::TransactionError::Transaction(e) => e,
            })?;

        Ok(())
    }

    /// Deletes all entries for a specific keyfact key.
    pub async fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, MemoryError> {
        let res = entities::conversation_keyfacts::Entity::delete_many()
            .filter(entities::conversation_keyfacts::Column::CardName.eq(card_name))
            .filter(entities::conversation_keyfacts::Column::Key.eq(key))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected as usize)
    }

    /// Counts the number of distinct keyfacts for a card.
    pub async fn count_keyfacts(&self, card_name: &str) -> Result<i64, MemoryError> {
        let count = entities::conversation_keyfacts::Entity::find()
            .filter(entities::conversation_keyfacts::Column::CardName.eq(card_name))
            .select_only()
            .column(entities::conversation_keyfacts::Column::Key)
            .distinct()
            .count(&self.db)
            .await?;
        Ok(count as i64)
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

    // ── Tool Embeddings (multi-vector) ──────────────────────────────────────

    /// Inserts or updates one field's embedding for a tool.
    ///
    /// `field` must be one of `"summary"`, `"description"`, `"capability"`,
    /// `"example"`, or `"negative"`, matching `ene_provider::EmbeddingKind`.
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

    /// Recalls both relevant conversation summaries and key facts for a card in a single call.
    ///
    /// Combines [`search_summaries`](Self::search_summaries) and
    /// [`get_all_keyfacts`](Self::get_all_keyfacts) for convenient prompt context assembly.
    pub async fn recall_context(
        &self,
        card_name: &str,
        query_embedding: &[f32],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<(Vec<RecalledSummary>, Vec<KeyFact>), MemoryError> {
        if self.is_legacy_migrated(card_name).await? {
            return Ok((vec![], vec![]));
        }

        let (summaries_result, key_facts_result) = tokio::join!(
            self.search_summaries(query_embedding, card_name, limit, similarity_threshold),
            self.get_all_keyfacts(card_name),
        );
        let summaries = summaries_result?;
        let key_facts = key_facts_result?;
        Ok((summaries, key_facts))
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
                self.transition_typed_memory_status(model.id, crate::MemoryStatus::Faded)
                    .await?;
                self.transition_typed_memory_status(model.id, crate::MemoryStatus::Archived)
                    .await?;
                archived += 1;
            }
        }
        Ok(archived)
    }

    /// Search typed memories by cosine similarity via content embeddings.
    ///
    /// Legacy vector-only search over `active` memories.
    pub async fn search_typed_memories(
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
            use sea_orm::Condition;
            select = select.filter(
                Condition::any()
                    .add(entities::typed_memories::Column::UserId.eq(uid))
                    .add(entities::typed_memories::Column::UserId.eq("")),
            );
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
                    },
                    row.similarity as f32,
                ))
            })
            .collect()
    }

    /// Hybrid search over typed memories with explainable score breakdown.
    ///
    /// Gathers candidates from recallable vector similarity, lexical token
    /// matches, a limited recent fallback, and active commitments; scores and
    /// de-duplicates by memory id; returns the top `options.limit` results.
    pub async fn search_typed_memories_hybrid(
        &self,
        options: &crate::MemorySearchOptions<'_>,
    ) -> Result<Vec<crate::ScoredMemory>, MemoryError> {
        use crate::search::{lexical_overlap_score, score_candidate};
        use crate::typed_memory::MemoryCandidateSource;
        use std::collections::HashMap;

        validate_embedding(options.query_embedding, self.embedding_dim)?;

        let pool = options.candidate_pool_size.max(options.limit);
        let recallable_statuses = [
            crate::MemoryStatus::Active.as_str(),
            crate::MemoryStatus::Faded.as_str(),
            crate::MemoryStatus::Disputed.as_str(),
        ];
        let mut gathered: HashMap<i64, crate::search::GatheredCandidate> = HashMap::new();

        // Vector candidates across recallable statuses.
        let vector_hits = self
            .search_typed_memories_vector(
                options.query_embedding,
                options.character_id,
                options.model_name,
                options.user_id,
                &recallable_statuses,
                pool,
                options.similarity_threshold,
            )
            .await?;
        for (item, similarity) in vector_hits {
            merge_hybrid_candidate(
                &mut gathered,
                options.user_id,
                item,
                similarity,
                MemoryCandidateSource::Vector,
            );
        }

        // Lexical candidates from token-based DB lookup.
        let lexical_candidates = self
            .list_lexical_typed_memory_candidates(
                options.query_text,
                options.character_id,
                options.user_id,
                pool,
            )
            .await?;
        for item in lexical_candidates {
            let lexical = lexical_overlap_score(options.query_text, &item.title, &item.content);
            if lexical > 0.0 {
                merge_hybrid_candidate(
                    &mut gathered,
                    options.user_id,
                    item,
                    0.0,
                    MemoryCandidateSource::Lexical,
                );
            }
        }

        // Active commitment memories (ledger-linked + commitment kind).
        let commitments = self
            .list_active_commitments(options.character_id, options.user_id, pool)
            .await?;
        let commitment_memory_ids: Vec<i64> = commitments
            .iter()
            .filter_map(|commitment| commitment.source_memory_id)
            .collect();
        let commitment_memories = self
            .get_typed_memories_by_ids(&commitment_memory_ids)
            .await?;
        for item in commitment_memories {
            merge_hybrid_candidate(
                &mut gathered,
                options.user_id,
                item,
                0.0,
                MemoryCandidateSource::Commitment,
            );
        }

        let commitment_kind = self
            .get_typed_memories_by_character(
                options.character_id,
                Some(crate::MemoryKind::Commitment),
                pool,
                0,
            )
            .await?;
        for item in commitment_kind {
            if item.status == crate::MemoryStatus::Active {
                merge_hybrid_candidate(
                    &mut gathered,
                    options.user_id,
                    item,
                    0.0,
                    MemoryCandidateSource::Commitment,
                );
            }
        }

        // Limited recent fallback for memories not already gathered.
        if options.recent_fallback_limit > 0 {
            let recent_candidates = self
                .list_recallable_typed_memories(
                    options.character_id,
                    options.user_id,
                    options.recent_fallback_limit.saturating_mul(2).max(pool),
                )
                .await?;
            let mut recent_added = 0usize;
            for item in recent_candidates {
                if recent_added >= options.recent_fallback_limit {
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
                    options.user_id,
                    item,
                    0.0,
                    MemoryCandidateSource::Recent,
                );
                recent_added += 1;
            }
        }

        let mut scored: Vec<crate::ScoredMemory> = gathered
            .into_values()
            .map(|candidate| {
                let breakdown = score_candidate(options, &candidate);
                crate::ScoredMemory {
                    item: candidate.item,
                    breakdown,
                    sources: candidate.sources,
                }
            })
            .filter(|scored| scored.breakdown.total >= options.min_score)
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

        scored.truncate(options.limit);
        Ok(scored)
    }

    /// List typed memories eligible for hybrid recall (`active`, `faded`, `disputed`).
    pub async fn list_recallable_typed_memories(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let statuses = [
            crate::MemoryStatus::Active.as_str(),
            crate::MemoryStatus::Faded.as_str(),
            crate::MemoryStatus::Disputed.as_str(),
        ];

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(statuses));

        if let Some(uid) = user_id {
            use sea_orm::Condition;
            query = query.filter(
                Condition::any()
                    .add(entities::typed_memories::Column::UserId.eq(uid))
                    .add(entities::typed_memories::Column::UserId.eq("")),
            );
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

    /// Fetch typed memories by primary key.
    async fn get_typed_memories_by_ids(
        &self,
        ids: &[i64],
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{EntityTrait, QueryFilter};

        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let models = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::Id.is_in(ids.to_vec()))
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

        let statuses = [
            crate::MemoryStatus::Active.as_str(),
            crate::MemoryStatus::Faded.as_str(),
            crate::MemoryStatus::Disputed.as_str(),
        ];

        let mut lexical_match = Condition::any();
        for token in tokens {
            let pattern = format!("%{token}%");
            lexical_match = lexical_match
                .add(entities::typed_memories::Column::Title.like(&pattern))
                .add(entities::typed_memories::Column::Content.like(&pattern));
        }

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(statuses))
            .filter(lexical_match);

        if let Some(uid) = user_id {
            query = query.filter(
                Condition::any()
                    .add(entities::typed_memories::Column::UserId.eq(uid))
                    .add(entities::typed_memories::Column::UserId.eq("")),
            );
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

    /// Transition a typed memory to a new lifecycle status.
    pub async fn update_typed_memory_status(
        &self,
        id: i64,
        new_status: crate::MemoryStatus,
    ) -> Result<bool, MemoryError> {
        self.transition_typed_memory_status(id, new_status).await
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
    pub async fn transition_typed_memory_status(
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
        self.transition_typed_memory_status(id, crate::MemoryStatus::UserDeleted)
            .await
    }

    /// List typed memories for the memory journal with user/scope and status filters.
    pub async fn list_journal_memories(
        &self,
        options: &crate::MemoryJournalListOptions<'_>,
    ) -> Result<Vec<crate::MemoryItem>, MemoryError> {
        use sea_orm::{Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut allowed_statuses = vec![
            crate::MemoryStatus::Active.as_str(),
            crate::MemoryStatus::Faded.as_str(),
            crate::MemoryStatus::Disputed.as_str(),
        ];
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
            query = query.filter(
                Condition::any()
                    .add(entities::typed_memories::Column::UserId.eq(uid))
                    .add(entities::typed_memories::Column::UserId.eq("")),
            );
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
            use sea_orm::Condition;
            query = query.filter(
                Condition::any()
                    .add(entities::typed_memories::Column::UserId.eq(uid))
                    .add(entities::typed_memories::Column::UserId.eq("")),
            );
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
            self.transition_typed_memory_status(id, target).await?;
            match target {
                crate::MemoryStatus::Faded => report.faded_count += 1,
                crate::MemoryStatus::Archived => report.archived_count += 1,
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
        let anchor = Utc::now() - chrono::Duration::days(days_ago);
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

    // ── Legacy Migration & Memory Spans (#98) ─────────────────────────────────

    /// Count legacy rows for a character card.
    pub async fn count_legacy_rows(
        &self,
        card_name: &str,
    ) -> Result<crate::LegacyRowCounts, MemoryError> {
        use sea_orm::PaginatorTrait;

        let summaries = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .count(&self.db)
            .await? as i64;

        let keyfacts = entities::conversation_keyfacts::Entity::find()
            .filter(entities::conversation_keyfacts::Column::CardName.eq(card_name))
            .count(&self.db)
            .await? as i64;

        let logs = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::CardName.eq(card_name))
            .count(&self.db)
            .await? as i64;

        Ok(crate::LegacyRowCounts {
            summaries,
            keyfacts,
            logs,
        })
    }

    /// Returns migration metadata when the card has been migrated.
    pub async fn get_migration_status(
        &self,
        card_name: &str,
    ) -> Result<Option<crate::MigrationStatus>, MemoryError> {
        let row = entities::memory_migration_meta::Entity::find_by_id(card_name.to_string())
            .one(&self.db)
            .await?;

        Ok(row.map(|m| crate::MigrationStatus {
            card_name: m.card_name,
            migrated_at: m.migrated_at,
            legacy_summaries_count: m.legacy_summaries_count,
            legacy_keyfacts_count: m.legacy_keyfacts_count,
            legacy_logs_count: m.legacy_logs_count,
            strategy: m.strategy,
        }))
    }

    /// Returns true when one-shot migration has completed for the card.
    pub async fn is_legacy_migrated(&self, card_name: &str) -> Result<bool, MemoryError> {
        Ok(self.get_migration_status(card_name).await?.is_some())
    }

    /// Record migration completion for a character card.
    pub async fn mark_migration_complete(
        &self,
        card_name: &str,
        counts: crate::LegacyRowCounts,
        strategy: &str,
    ) -> Result<(), MemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let active = entities::memory_migration_meta::ActiveModel {
            card_name: Set(card_name.to_string()),
            migrated_at: Set(now),
            legacy_summaries_count: Set(counts.summaries as i32),
            legacy_keyfacts_count: Set(counts.keyfacts as i32),
            legacy_logs_count: Set(counts.logs as i32),
            strategy: Set(strategy.to_string()),
        };
        active.insert(&self.db).await?;
        Ok(())
    }

    /// Run one-shot legacy → typed migration.
    pub async fn migrate_legacy(
        &self,
        options: &crate::LegacyMigrationOptions,
    ) -> Result<crate::LegacyMigrationReport, MemoryError> {
        crate::legacy_migration::execute_legacy_migration(self, options).await
    }

    /// Apply legacy migration writes inside a single database transaction.
    pub(crate) async fn apply_legacy_migration_writes(
        &self,
        options: &crate::LegacyMigrationOptions,
        counts: crate::LegacyRowCounts,
        summary_rows: Vec<crate::legacy_migration::LegacySummaryRow>,
        keyfact_rows: Vec<crate::legacy_migration::LegacyKeyfactRow>,
        spans: Vec<NewMemorySpan>,
    ) -> Result<crate::LegacyMigrationReport, MemoryError> {
        use sea_orm::TransactionTrait;

        let card_name = options.card_name.clone();
        let user_id = options.user_id.clone();
        let embedding_model = options.embedding_model.clone();
        let embedding_dim = self.embedding_dim;

        self.db
            .transaction::<_, crate::LegacyMigrationReport, MemoryError>(|txn| {
                let card_name = card_name.clone();
                let user_id = user_id.clone();
                let embedding_model = embedding_model.clone();
                Box::pin(async move {
                    let mut report = crate::LegacyMigrationReport {
                        summaries_migrated: 0,
                        keyfacts_migrated: 0,
                        spans_migrated: 0,
                        skipped_existing: 0,
                    };

                    for row in summary_rows {
                        let source_ref = format!("legacy:summary:{}", row.id);
                        if typed_memory_exists_by_source_ref_on(txn, &source_ref).await? {
                            report.skipped_existing += 1;
                            continue;
                        }

                        let item = crate::legacy_migration::summaries::summary_to_typed_memory(
                            &card_name,
                            &user_id,
                            &row.summary,
                            row.id,
                        );
                        let memory_id = insert_typed_memory_on(txn, &item).await?;
                        patch_typed_memory_created_at_on(txn, memory_id, row.created_at).await?;

                        let embedding = bytes_to_embedding(&row.embedding);
                        if !embedding.is_empty() {
                            upsert_memory_embedding_on(
                                txn,
                                embedding_dim,
                                memory_id,
                                &embedding_model,
                                "content",
                                &embedding,
                            )
                            .await?;
                        }
                        report.summaries_migrated += 1;
                    }

                    for row in keyfact_rows {
                        if row.value.is_empty() {
                            continue;
                        }
                        let source_ref = format!("legacy:keyfact:{}", row.id);
                        if typed_memory_exists_by_source_ref_on(txn, &source_ref).await? {
                            report.skipped_existing += 1;
                            continue;
                        }

                        let item = crate::legacy_migration::keyfacts::keyfact_to_typed_memory(
                            &card_name, &user_id, &row.key, &row.value, row.id,
                        );
                        let memory_id = insert_typed_memory_on(txn, &item).await?;
                        patch_typed_memory_created_at_on(txn, memory_id, row.created_at).await?;
                        report.keyfacts_migrated += 1;
                    }

                    for span in spans {
                        if memory_span_exists_on(txn, &span.session_id, span.turn_start).await? {
                            report.skipped_existing += 1;
                            continue;
                        }
                        insert_memory_span_on(txn, &span).await?;
                        report.spans_migrated += 1;
                    }

                    mark_migration_complete_on(txn, &card_name, counts, "one_shot").await?;
                    Ok(report)
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => {
                    MemoryError::MemoryStoreError(db_err)
                }
                sea_orm::TransactionError::Transaction(e) => e,
            })
    }

    /// Truncate legacy tables and typed memory for a card (destructive reset).
    pub async fn reset_legacy_memory(&self, card_name: &str) -> Result<(), MemoryError> {
        use sea_orm::TransactionTrait;

        let card_name = card_name.to_string();
        self.db
            .transaction::<_, (), MemoryError>(|txn| {
                let card_name = card_name.clone();
                Box::pin(async move {
                    let card = card_name.as_str();
                    entities::conversation_keyfacts::Entity::delete_many()
                        .filter(entities::conversation_keyfacts::Column::CardName.eq(card))
                        .exec(txn)
                        .await?;
                    entities::conversation_summaries::Entity::delete_many()
                        .filter(entities::conversation_summaries::Column::CardName.eq(card))
                        .exec(txn)
                        .await?;
                    let session_ids = list_session_ids_for_card_on_conn(txn, card).await?;
                    entities::conversation_logs::Entity::delete_many()
                        .filter(entities::conversation_logs::Column::CardName.eq(card))
                        .exec(txn)
                        .await?;
                    entities::memory_migration_meta::Entity::delete_by_id(card_name.clone())
                        .exec(txn)
                        .await?;
                    if !session_ids.is_empty() {
                        entities::memory_spans::Entity::delete_many()
                            .filter(entities::memory_spans::Column::SessionId.is_in(session_ids))
                            .exec(txn)
                            .await?;
                    }
                    entities::typed_memories::Entity::delete_many()
                        .filter(entities::typed_memories::Column::CharacterId.eq(card))
                        .exec(txn)
                        .await?;
                    Ok(())
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => {
                    MemoryError::MemoryStoreError(db_err)
                }
                sea_orm::TransactionError::Transaction(e) => e,
            })?;

        Ok(())
    }

    /// Returns an error when legacy data exists, migration is incomplete, and strict mode is on.
    pub async fn ensure_legacy_migration_allowed(
        &self,
        card_name: &str,
        require_migration: bool,
    ) -> Result<(), MemoryError> {
        if !require_migration {
            return Ok(());
        }
        let counts = self.count_legacy_rows(card_name).await?;
        if counts.requires_migration_gate() && !self.is_legacy_migrated(card_name).await? {
            return Err(MemoryError::LegacyMemoryNotMigrated {
                card_name: card_name.to_string(),
            });
        }
        Ok(())
    }

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

    pub(crate) async fn list_legacy_summaries(
        &self,
        card_name: &str,
    ) -> Result<Vec<crate::legacy_migration::LegacySummaryRow>, MemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .order_by_asc(entities::conversation_summaries::Column::Id)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| crate::legacy_migration::LegacySummaryRow {
                id: r.id,
                summary: r.summary,
                embedding: r.embedding,
                created_at: r.created_at,
            })
            .collect())
    }

    pub(crate) async fn list_legacy_keyfacts(
        &self,
        card_name: &str,
    ) -> Result<Vec<crate::legacy_migration::LegacyKeyfactRow>, MemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::conversation_keyfacts::Entity::find()
            .filter(entities::conversation_keyfacts::Column::CardName.eq(card_name))
            .order_by_asc(entities::conversation_keyfacts::Column::Id)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| crate::legacy_migration::LegacyKeyfactRow {
                id: r.id,
                key: r.key,
                value: r.value,
                created_at: r.created_at,
            })
            .collect())
    }

    pub(crate) async fn list_legacy_logs(
        &self,
        card_name: &str,
    ) -> Result<Vec<crate::legacy_migration::LegacyLogRowRaw>, MemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::CardName.eq(card_name))
            .order_by_asc(entities::conversation_logs::Column::Id)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| crate::legacy_migration::LegacyLogRowRaw {
                session_id: r.session_id,
                role: r.role,
                content: r.content,
            })
            .collect())
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
            source_memory_id: Set(item.source_memory_id),
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

    /// Look up a commitment linked to a typed memory row.
    pub async fn get_commitment_by_source_memory(
        &self,
        source_memory_id: i64,
    ) -> Result<Option<crate::Commitment>, MemoryError> {
        let maybe_model = entities::commitments::Entity::find()
            .filter(entities::commitments::Column::SourceMemoryId.eq(source_memory_id))
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
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let now = Utc::now();
        let maybe_model = entities::commitments::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        if crate::CommitmentStatus::from_db_str(&model.status) != crate::CommitmentStatus::Active {
            return Ok(false);
        }

        let mut active: entities::commitments::ActiveModel = model.into();
        active.status = Set(new_status.as_str().to_string());
        active.updated_at = Set(now);
        if new_status == crate::CommitmentStatus::Done {
            active.completed_at = Set(Some(now));
        }
        active.update(&self.db).await?;
        Ok(true)
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
                updated += 1;
            }
        }
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_bytes() {
        let original = vec![1.0_f32, 0.5, -0.25, 0.0];
        let bytes = embedding_to_bytes(&original);
        let restored = bytes_to_embedding(&bytes);
        for (a, b) in original.iter().zip(restored.iter()) {
            assert!((a - b).abs() < 1e-7, "Mismatch: {a} != {b}");
        }
    }

    #[tokio::test]
    async fn test_insert_and_search_summaries() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb_a = vec![1.0_f32, 0.0, 0.0, 0.0];
        let emb_b = vec![0.0_f32, 1.0, 0.0, 0.0];

        store
            .insert_summary("s1", "char", "Summary A", &[], &emb_a, Utc::now())
            .await
            .unwrap();
        store
            .insert_summary("s2", "char", "Summary B", &[], &emb_b, Utc::now())
            .await
            .unwrap();

        let results = store
            .search_summaries(&emb_a, "char", 5, 0.5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.summary, "Summary A");
        assert!((results[0].similarity - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn test_keyfacts_crud() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let summary_id = store
            .insert_summary("s1", "char", "Summary", &[], &emb, Utc::now())
            .await
            .unwrap();

        let facts = vec![
            KeyFact {
                key: "job".to_string(),
                value: "engineer".to_string(),
            },
            KeyFact {
                key: "food".to_string(),
                value: "ramen".to_string(),
            },
        ];
        let now = Utc::now();

        for f in &facts {
            let new_fact = entities::conversation_keyfacts::ActiveModel {
                card_name: sea_orm::ActiveValue::Set("char".to_string()),
                summary_id: sea_orm::ActiveValue::Set(Some(summary_id)),
                key: sea_orm::ActiveValue::Set(f.key.clone()),
                value: sea_orm::ActiveValue::Set(f.value.clone()),
                created_at: sea_orm::ActiveValue::Set(now.clone()),
                ..Default::default()
            };
            use sea_orm::ActiveModelTrait;
            new_fact.insert(&store.db).await.unwrap();
        }

        let all_facts = store.get_all_keyfacts("char").await.unwrap();
        assert_eq!(all_facts.len(), 2);
        assert_eq!(all_facts[0].key, "food");
        assert_eq!(all_facts[1].key, "job");

        store.upsert_keyfact("char", "food", "sushi").await.unwrap();
        let all_facts = store.get_all_keyfacts("char").await.unwrap();
        let food_fact = all_facts.iter().find(|f| f.key == "food").unwrap();
        assert_eq!(food_fact.value, "sushi");

        store.delete_keyfact("char", "food").await.unwrap();
        let all_facts = store.get_all_keyfacts("char").await.unwrap();
        assert_eq!(all_facts.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_summary_cascades() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let summary_id = store
            .insert_summary("s1", "char", "Summary", &[], &emb, Utc::now())
            .await
            .unwrap();

        let now = Utc::now();
        let new_fact = entities::conversation_keyfacts::ActiveModel {
            card_name: sea_orm::ActiveValue::Set("char".to_string()),
            summary_id: sea_orm::ActiveValue::Set(Some(summary_id)),
            key: sea_orm::ActiveValue::Set("job".to_string()),
            value: sea_orm::ActiveValue::Set("engineer".to_string()),
            created_at: sea_orm::ActiveValue::Set(now.clone()),
            ..Default::default()
        };
        use sea_orm::ActiveModelTrait;
        new_fact.insert(&store.db).await.unwrap();

        store.delete_summary(summary_id).await.unwrap();
        assert_eq!(store.count_summaries("char").await.unwrap(), 0);
        assert_eq!(store.count_keyfacts("char").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_insert_summary_with_empty_value_deletes_keyfact() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let summary_id = store
            .insert_summary("s1", "char", "Summary", &[], &emb, Utc::now())
            .await
            .unwrap();

        let now = Utc::now();
        for (k, v) in &[("job", "engineer"), ("hobby", "guitar")] {
            let new_fact = entities::conversation_keyfacts::ActiveModel {
                card_name: sea_orm::ActiveValue::Set("char".to_string()),
                summary_id: sea_orm::ActiveValue::Set(Some(summary_id)),
                key: sea_orm::ActiveValue::Set(k.to_string()),
                value: sea_orm::ActiveValue::Set(v.to_string()),
                created_at: sea_orm::ActiveValue::Set(now.clone()),
                ..Default::default()
            };
            use sea_orm::ActiveModelTrait;
            new_fact.insert(&store.db).await.unwrap();
        }

        assert_eq!(store.get_all_keyfacts("char").await.unwrap().len(), 2);

        let emb2 = vec![0.0_f32, 1.0, 0.0, 0.0];
        store
            .insert_summary(
                "s2",
                "char",
                "Summary 2",
                &[
                    KeyFact {
                        key: "job".to_string(),
                        value: "designer".to_string(),
                    },
                    KeyFact {
                        key: "hobby".to_string(),
                        value: String::new(),
                    },
                ],
                &emb2,
                Utc::now(),
            )
            .await
            .unwrap();

        let all_facts = store.get_all_keyfacts("char").await.unwrap();
        assert_eq!(all_facts.len(), 1);
        assert_eq!(all_facts[0].key, "job");
        assert_eq!(all_facts[0].value, "designer");
    }

    #[tokio::test]
    async fn test_tool_embedding_field_upsert_and_list() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
    async fn test_delete_tool_embeddings_cascades() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
    async fn insert_summary_rejects_bad_embedding() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();
        let facts: Vec<KeyFact> = vec![];

        // Length mismatch.
        let wrong_len = vec![0.1, 0.2, 0.3];
        let err = store
            .insert_summary("s", "c", "summary", &facts, &wrong_len, now)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::InvalidEmbedding(_)),
            "expected InvalidEmbedding, got {err:?}"
        );

        // NaN.
        let with_nan = vec![0.1, f32::NAN, 0.3, 0.4];
        let err = store
            .insert_summary("s", "c", "summary", &facts, &with_nan, now)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::InvalidEmbedding(_)),
            "expected InvalidEmbedding, got {err:?}"
        );

        // Infinity.
        let with_inf = vec![0.1, 0.2, f32::INFINITY, 0.4];
        let err = store
            .insert_summary("s", "c", "summary", &facts, &with_inf, now)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::InvalidEmbedding(_)),
            "expected InvalidEmbedding, got {err:?}"
        );

        // Valid embedding still works.
        let ok = vec![0.1, 0.2, 0.3, 0.4];
        store
            .insert_summary("s", "c", "summary", &facts, &ok, now)
            .await
            .unwrap();
    }

    /// Regression test for #41 (bug 1): the memory store
    /// must apply `foreign_keys=ON` (and the other
    /// safety PRAGMAs) on every connection it opens. For
    /// an in-memory store `journal_mode=WAL` is a no-op
    /// (SQLite returns `memory`), but `foreign_keys` and
    /// `busy_timeout` are still meaningful.
    #[tokio::test]
    async fn pragmas_are_applied_on_open() {
        use sea_orm::ConnectionTrait;
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
    async fn test_insert_conversation_turn() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let session_id = "turn-test-session";

        let ids = store
            .insert_conversation_turn(session_id, "ene", "Hello", "Hi there!")
            .await
            .unwrap();
        let logs = store.get_logs_by_session(session_id).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].0, "user");
        assert_eq!(logs[0].1, "Hello");
        assert_eq!(logs[1].0, "assistant");
        assert_eq!(logs[1].1, "Hi there!");
        let _ = ids;
    }

    #[tokio::test]
    async fn affect_state_get_upsert() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

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
        assert_eq!(loaded.discrete_emotions[0].label, "joy");
        assert!((loaded.discrete_emotions[0].intensity - 0.8).abs() < f32::EPSILON);

        state.valence = -0.2;
        state.discrete_emotions = vec![crate::DiscreteEmotion::new("sadness", 0.6)];
        store.upsert_affect_state(&state).await.unwrap();

        let loaded2 = store.get_affect_state("ene").await.unwrap();
        assert!((loaded2.valence + 0.2).abs() < f32::EPSILON);
        assert_eq!(loaded2.discrete_emotions.len(), 1);
        assert_eq!(loaded2.discrete_emotions[0].label, "sadness");
    }

    #[tokio::test]
    async fn typed_memory_crud() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

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
            .update_typed_memory_status(id, crate::MemoryStatus::Faded)
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
                .update_typed_memory_status(999_999, crate::MemoryStatus::Active)
                .await
                .unwrap()
        );
        assert!(!store.bump_typed_memory_access(999_999).await.unwrap());
    }

    #[tokio::test]
    async fn typed_memory_search_with_embedding() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

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
        assert!((results[0].1 - 1.0).abs() < f32::EPSILON);
        assert_eq!(results[0].0.title, "Test memory");
    }

    #[tokio::test]
    async fn supersede_typed_memory_links_rows() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

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
        let store = MemoryStore::open_in_memory(4).await.unwrap();

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
        };
        let old_id = store.insert_typed_memory(&item).await.unwrap();
        store
            .update_typed_memory_status(old_id, crate::MemoryStatus::UserDeleted)
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        let memory_id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Commitment,
                title: "design review".into(),
                content: "Discuss the design next session".into(),
                source: crate::MemorySource::Inferred,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.8),
                salience: crate::MemorySalience::new(0.7),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
            })
            .await
            .unwrap();

        let id = store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "design review".into(),
                description: "Discuss the design next session".into(),
                status: crate::CommitmentStatus::Active,
                due_at: None,
                due_label: Some("next session".into()),
                source_memory_id: Some(memory_id),
            })
            .await
            .unwrap();

        let loaded = store.get_commitment(id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "design review");
        assert_eq!(loaded.source_memory_id, Some(memory_id));

        let by_source = store
            .get_commitment_by_source_memory(memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_source.id, Some(id));

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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
                source_memory_id: None,
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
                source_memory_id: None,
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
                source_memory_id: None,
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
                source_memory_id: None,
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
                source_memory_id: None,
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
                source_memory_id: None,
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
    ) -> crate::MemorySearchOptions<'a> {
        crate::MemorySearchOptions {
            query_text,
            query_embedding,
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].item.title, "important fact");
        assert!(results[0].breakdown.total >= results[1].breakdown.total);
    }

    #[tokio::test]
    async fn hybrid_search_lexical_component_for_matching_query() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };
        insert_memory_with_embedding(&store, &item, &orthogonal).await;

        let options = hybrid_search_options("matcha latte", &orthogonal, now);
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].breakdown.lexical_score > 0.0);
        assert!(
            results[0]
                .sources
                .contains(&crate::MemoryCandidateSource::Lexical)
        );
    }

    #[tokio::test]
    async fn hybrid_search_surfaces_active_commitment_with_low_vector_similarity() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();
        let query_emb = vec![1.0, 0.0, 0.0, 0.0];
        let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

        let memory_id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Commitment,
                title: "follow up".into(),
                content: "Review the architecture document".into(),
                source: crate::MemorySource::Inferred,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.8),
                salience: crate::MemorySalience::new(0.5),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
            })
            .await
            .unwrap();
        store
            .upsert_memory_embedding(memory_id, "test-model", "content", &orthogonal)
            .await
            .unwrap();
        store
            .insert_commitment(&crate::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "follow up".into(),
                description: "Review the architecture document".into(),
                status: crate::CommitmentStatus::Active,
                due_at: None,
                due_label: Some("next time".into()),
                source_memory_id: Some(memory_id),
            })
            .await
            .unwrap();

        let options = hybrid_search_options("unrelated query", &query_emb, now);
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.item.id == Some(memory_id) && r.breakdown.commitment_boost > 0.0),
            "active commitment should be recalled despite low vector similarity"
        );
    }

    #[tokio::test]
    async fn hybrid_search_excludes_archived_superseded_and_user_deleted() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };

        for status in [
            crate::MemoryStatus::Archived,
            crate::MemoryStatus::Superseded,
            crate::MemoryStatus::UserDeleted,
        ] {
            let mut item = base.clone();
            item.title = format!("{:?}", status);
            item.status = status;
            insert_memory_with_embedding(&store, &item, &emb).await;
        }

        let options = hybrid_search_options("shared content", &emb, now);
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_search_faded_memory_has_stale_penalty() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };
        insert_memory_with_embedding(&store, &item, &emb).await;

        let options = hybrid_search_options("faded memory", &emb, now);
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].breakdown.vector_similarity > 0.0);
        assert!(results[0].breakdown.stale_penalty > 0.0);
    }

    #[tokio::test]
    async fn hybrid_search_finds_old_lexical_match_outside_recent_pool() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
            };
            insert_memory_with_embedding(&store, &filler, &orthogonal).await;
        }

        let mut options = hybrid_search_options("ancient dragon recipe", &query_emb, now);
        options.recent_fallback_limit = 0;
        options.similarity_threshold = 0.8;
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.item.id == Some(old_id) && r.breakdown.lexical_score > 0.0),
            "old lexical match should be found outside the recent pool"
        );
    }

    #[tokio::test]
    async fn hybrid_search_excludes_unrelated_recent_without_fallback() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };
        insert_memory_with_embedding(&store, &unrelated, &orthogonal).await;

        let mut options = hybrid_search_options("completely different topic", &query_emb, now);
        options.recent_fallback_limit = 0;
        options.similarity_threshold = 0.8;
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_search_ranks_higher_confidence_when_other_signals_match() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].item.title, "shared topic high");
        assert!(results[0].breakdown.confidence > results[1].breakdown.confidence);
    }

    #[tokio::test]
    async fn hybrid_search_respects_user_id_scope() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };

        let mut user1_item = base.clone();
        user1_item.user_id = "user1".into();
        let mut user2_item = base;
        user2_item.user_id = "user2".into();

        insert_memory_with_embedding(&store, &user1_item, &query_emb).await;
        insert_memory_with_embedding(&store, &user2_item, &query_emb).await;

        let mut options = hybrid_search_options("scoped memory", &query_emb, now);
        options.user_id = Some("user1");
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item.user_id, "user1");
    }

    #[tokio::test]
    async fn hybrid_search_dedupes_multi_source_candidates() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };
        insert_memory_with_embedding(&store, &item, &emb).await;

        let options = hybrid_search_options("pizza tradition", &emb, now);
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].sources.len() >= 2);
    }

    #[tokio::test]
    async fn transition_typed_memory_status_rejects_invalid_edge() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
            })
            .await
            .unwrap();

        let err = store
            .transition_typed_memory_status(id, crate::MemoryStatus::Active)
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
            })
            .await
            .unwrap();
        store.test_backdate_typed_memory(id, 45).await.unwrap();

        let before = store.get_typed_memory(id).await.unwrap().unwrap();
        assert!(
            store
                .transition_typed_memory_status(id, crate::MemoryStatus::Faded)
                .await
                .unwrap()
        );

        let after = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(after.status, crate::MemoryStatus::Faded);
        assert_eq!(after.faded_at, Some(before.updated_at));
    }

    #[tokio::test]
    async fn single_row_natural_decay_reaches_archived_in_two_passes() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        let store = MemoryStore::open_in_memory(4).await.unwrap();
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
        };
        insert_memory_with_embedding(&store, &item, &emb).await;

        let options = hybrid_search_options("pinned vector", &emb, now);
        let results = store.search_typed_memories_hybrid(&options).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].item.pinned);
    }

    #[tokio::test]
    async fn legacy_migration_summaries_to_episodic() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        store
            .insert_summary("s1", "char", "Legacy summary text", &[], &emb, Utc::now())
            .await
            .unwrap();

        let report = store
            .migrate_legacy(&crate::LegacyMigrationOptions {
                card_name: "char".into(),
                user_id: "user".into(),
                embedding_model: "test".into(),
                dry_run: false,
            })
            .await
            .unwrap();

        assert_eq!(report.summaries_migrated, 1);
        let typed = store
            .get_typed_memories_by_character("char", Some(crate::MemoryKind::Episodic), 10, 0)
            .await
            .unwrap();
        assert_eq!(typed.len(), 1);
        assert!(typed[0].content.contains("Legacy summary"));
        assert!(store.is_legacy_migrated("char").await.unwrap());
    }

    #[tokio::test]
    async fn legacy_write_forbidden_in_read_only_mode() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        store.set_legacy_write_mode(LegacyWriteMode::ReadOnly);
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let err = store
            .insert_summary("s1", "char", "blocked", &[], &emb, Utc::now())
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::LegacyWriteForbidden));
    }

    #[tokio::test]
    async fn migration_idempotent_skips_existing_source_ref() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        store
            .insert_summary("s1", "char", "Once", &[], &emb, Utc::now())
            .await
            .unwrap();

        let options = crate::LegacyMigrationOptions {
            card_name: "char".into(),
            user_id: "user".into(),
            embedding_model: "test".into(),
            dry_run: false,
        };
        store.migrate_legacy(&options).await.unwrap();
        let err = store.migrate_legacy(&options).await.unwrap_err();
        assert!(matches!(err, MemoryError::LegacyAlreadyMigrated { .. }));
    }

    #[tokio::test]
    async fn reset_legacy_clears_tables() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        store
            .insert_summary("s1", "char", "x", &[], &emb, Utc::now())
            .await
            .unwrap();
        store.reset_legacy_memory("char").await.unwrap();
        let counts = store.count_legacy_rows("char").await.unwrap();
        assert_eq!(counts.summaries, 0);
        assert_eq!(counts.keyfacts, 0);
    }

    #[tokio::test]
    async fn reset_legacy_preserves_other_card_spans() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        store
            .insert_log("s-char", "char", "user", "hello")
            .await
            .unwrap();
        store
            .insert_log("s-other", "other", "user", "hi")
            .await
            .unwrap();
        store
            .insert_memory_span(&NewMemorySpan {
                session_id: "s-char".into(),
                turn_start: 0,
                turn_end: 0,
                raw_excerpt: Some("char span".into()),
                compressed_summary: None,
                compression_level: 0,
            })
            .await
            .unwrap();
        store
            .insert_memory_span(&NewMemorySpan {
                session_id: "s-other".into(),
                turn_start: 0,
                turn_end: 0,
                raw_excerpt: Some("other span".into()),
                compressed_summary: None,
                compression_level: 0,
            })
            .await
            .unwrap();

        store.reset_legacy_memory("char").await.unwrap();

        let char_spans = store.list_memory_spans_by_session("s-char").await.unwrap();
        let other_spans = store.list_memory_spans_by_session("s-other").await.unwrap();
        assert!(char_spans.is_empty());
        assert_eq!(other_spans.len(), 1);
    }

    #[tokio::test]
    async fn require_migration_allows_logs_only() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        store
            .insert_log("s1", "char", "user", "hello")
            .await
            .unwrap();
        store
            .ensure_legacy_migration_allowed("char", true)
            .await
            .expect("logs alone should not block");
    }

    #[tokio::test]
    async fn require_migration_blocks_unmigrated_summaries() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        store
            .insert_summary("s1", "char", "summary", &[], &emb, Utc::now())
            .await
            .unwrap();
        let err = store
            .ensure_legacy_migration_allowed("char", true)
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::LegacyMemoryNotMigrated { .. }));
    }

    #[tokio::test]
    async fn legacy_migration_keyfacts_to_preference_and_profile() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        store
            .upsert_keyfact("char", "pref_color", "blue")
            .await
            .unwrap();
        store
            .upsert_keyfact("char", "job", "engineer")
            .await
            .unwrap();

        let report = store
            .migrate_legacy(&crate::LegacyMigrationOptions {
                card_name: "char".into(),
                user_id: "user".into(),
                embedding_model: "test".into(),
                dry_run: false,
            })
            .await
            .unwrap();

        assert_eq!(report.keyfacts_migrated, 2);
        let prefs = store
            .get_typed_memories_by_character("char", Some(crate::MemoryKind::Preference), 10, 0)
            .await
            .unwrap();
        let profiles = store
            .get_typed_memories_by_character("char", Some(crate::MemoryKind::UserProfile), 10, 0)
            .await
            .unwrap();
        assert_eq!(prefs.len(), 1);
        assert_eq!(profiles.len(), 1);
    }

    #[tokio::test]
    async fn legacy_migration_logs_to_spans() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        store
            .insert_log("s1", "char", "user", "hello")
            .await
            .unwrap();
        store
            .insert_log("s1", "char", "assistant", "hi there")
            .await
            .unwrap();

        let report = store
            .migrate_legacy(&crate::LegacyMigrationOptions {
                card_name: "char".into(),
                user_id: "user".into(),
                embedding_model: "test".into(),
                dry_run: false,
            })
            .await
            .unwrap();

        assert_eq!(report.spans_migrated, 1);
        let spans = store.list_memory_spans_by_session("s1").await.unwrap();
        assert_eq!(spans.len(), 1);
        let excerpt = spans[0].raw_excerpt.as_ref().unwrap();
        assert!(excerpt.contains("hello"));
        assert!(excerpt.contains("hi there"));
    }

    #[tokio::test]
    async fn legacy_migration_span_idempotent_on_retry_before_marker() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        store
            .insert_log("s1", "char", "user", "once")
            .await
            .unwrap();

        let options = crate::LegacyMigrationOptions {
            card_name: "char".into(),
            user_id: "user".into(),
            embedding_model: "test".into(),
            dry_run: false,
        };

        let first = store.migrate_legacy(&options).await.unwrap();
        assert_eq!(first.spans_migrated, 1);

        entities::memory_migration_meta::Entity::delete_by_id("char".to_string())
            .exec(store.connection())
            .await
            .unwrap();

        let second = store.migrate_legacy(&options).await.unwrap();
        assert_eq!(second.spans_migrated, 0);
        assert_eq!(second.skipped_existing, 1);
    }
}
