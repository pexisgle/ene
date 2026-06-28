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
    b.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
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
///   constraints, in particular
///   `conversation_keyfacts.summary_id` →
///   `conversation_summaries.id`.
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

impl MemoryStore {
    fn init(db: DatabaseConnection, embedding_dim: usize) -> Self {
        Self { db, embedding_dim }
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
    /// spurious `database is locked` errors under contention; and the FK
    /// pragma makes the `conversation_keyfacts.summary_id` reference
    /// actually enforced.
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
        let similarity_expr = Expr::cust_with_values(
            "1.0 - vec_distance_cosine(embedding, ?)",
            vec![query_bytes.clone()],
        );

        // TODO: refactor the threshold filter to reference
        // the projected `similarity` column once the
        // SeaORM `expr_as` / `Expr::col` API supports an
        // `IdenStatic` alias. Today, sea-orm's
        // `SimpleExpr` lacks a `gte` method, so the
        // filter has to re-evaluate the expression.
        let select = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .expr_as(similarity_expr, "similarity")
            .filter(Expr::cust_with_values(
                "1.0 - vec_distance_cosine(embedding, ?) >= ?",
                vec![
                    sea_orm::Value::from(query_bytes),
                    sea_orm::Value::from(f64::from(similarity_threshold)),
                ],
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

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = Expr::cust_with_values(
            "1.0 - vec_distance_cosine(embedding, ?)",
            vec![query_bytes.clone()],
        );

        let factor = 4u64;
        let row_cap = (limit as u64).saturating_mul(factor).max(limit as u64);

        let select = entities::tool_embedding_index::Entity::find()
            .select_only()
            .column(entities::tool_embedding_index::Column::ToolName)
            .expr_as(similarity_expr, "similarity")
            .filter(Expr::cust_with_values(
                "1.0 - vec_distance_cosine(embedding, ?) >= ?",
                vec![
                    sea_orm::Value::from(query_bytes),
                    sea_orm::Value::from(f64::from(similarity_threshold)),
                ],
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
                    serde_json::from_str(&model.discrete_emotions).unwrap_or_default();
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
                })
            }
            None => Ok(crate::AffectState::neutral(character_id)),
        }
    }

    /// Persist or update an [`crate::AffectState`].
    pub async fn upsert_affect_state(&self, state: &crate::AffectState) -> Result<(), MemoryError> {
        use entities::affect_states::{ActiveModel, Entity};
        use sea_orm::EntityTrait;

        let now = Utc::now();
        let discrete_json = serde_json::to_string(&state.discrete_emotions)
            .map_err(|e| MemoryError::Other(e.to_string()))?;

        let existing = Entity::find_by_id(&state.character_id)
            .one(&self.db)
            .await?;

        if let Some(model) = existing {
            let mut active: ActiveModel = model.into();
            active.user_id = sea_orm::Set(state.user_id.clone());
            active.valence = sea_orm::Set(state.valence);
            active.arousal = sea_orm::Set(state.arousal);
            active.dominance = sea_orm::Set(state.dominance);
            active.trust = sea_orm::Set(state.trust);
            active.affinity = sea_orm::Set(state.affinity);
            active.irritation = sea_orm::Set(state.irritation);
            active.curiosity = sea_orm::Set(state.curiosity);
            active.fatigue = sea_orm::Set(state.fatigue);
            active.mood_label = sea_orm::Set(state.mood_label.clone());
            active.last_expression = sea_orm::Set(state.last_expression.clone());
            active.discrete_emotions = sea_orm::Set(discrete_json);
            active.updated_at = sea_orm::Set(now);
            Entity::update(active).exec(&self.db).await?;
        } else {
            let active = ActiveModel {
                character_id: sea_orm::Set(state.character_id.clone()),
                user_id: sea_orm::Set(state.user_id.clone()),
                valence: sea_orm::Set(state.valence),
                arousal: sea_orm::Set(state.arousal),
                dominance: sea_orm::Set(state.dominance),
                trust: sea_orm::Set(state.trust),
                affinity: sea_orm::Set(state.affinity),
                irritation: sea_orm::Set(state.irritation),
                curiosity: sea_orm::Set(state.curiosity),
                fatigue: sea_orm::Set(state.fatigue),
                mood_label: sea_orm::Set(state.mood_label.clone()),
                last_expression: sea_orm::Set(state.last_expression.clone()),
                discrete_emotions: sea_orm::Set(discrete_json),
                updated_at: sea_orm::Set(now),
            };
            Entity::insert(active).exec(&self.db).await?;
        }
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
            created_at: Set(now),
            updated_at: Set(now),
            valid_from: Set(item.valid_from),
            valid_until: Set(item.valid_until),
            status: Set(item.status.as_str().to_string()),
            supersedes_id: Set(item.supersedes_id),
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

    /// Search typed memories by cosine similarity via content embeddings.
    pub async fn search_typed_memories(
        &self,
        query_embedding: &[f32],
        character_id: &str,
        model_name: &str,
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
            similarity: f64,
        }

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = Expr::cust_with_values(
            "1.0 - vec_distance_cosine(memory_embeddings.embedding, ?)",
            vec![query_bytes.clone()],
        );

        let threshold_val = f64::from(similarity_threshold);
        let limit_val = limit as u64;

        let select = entities::memory_embeddings::Entity::find()
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
            .expr_as(similarity_expr, "similarity")
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.eq("active"))
            .filter(entities::memory_embeddings::Column::ModelName.eq(model_name))
            .filter(entities::memory_embeddings::Column::Field.eq("content"))
            .filter(Expr::cust_with_values(
                "1.0 - vec_distance_cosine(memory_embeddings.embedding, ?) >= ?",
                vec![
                    sea_orm::Value::from(query_bytes),
                    sea_orm::Value::from(threshold_val),
                ],
            ))
            .order_by_desc(Expr::col("similarity"))
            .limit(limit_val);

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
                    },
                    row.similarity as f32,
                ))
            })
            .collect()
    }

    /// Transition a typed memory to a new lifecycle status.
    pub async fn update_typed_memory_status(
        &self,
        id: i64,
        new_status: crate::MemoryStatus,
    ) -> Result<bool, MemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let now = Utc::now();
        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.status = Set(new_status.as_str().to_string());
        active.updated_at = Set(now);
        active.update(&self.db).await?;
        Ok(true)
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
}
