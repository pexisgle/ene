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

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
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
    pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| MemoryError::MemoryStoreConnectionError("Invalid path".to_string()))?;
        init_sqlite_vec();
        let opt = ConnectOptions::new(format!("sqlite:{path_str}"));
        let db = Database::connect(opt)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Migrator::up(&db, None)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Ok(Self::init(db, embedding_dim))
    }

    /// Opens an in-memory memory store (useful for testing).
    ///
    /// Registers the `sqlite-vec` extension process-globally *before* opening
    /// the connection, since `:memory:` reuses a single persistent connection
    /// for the life of the store. Uses `"sqlite::memory:"` as the database
    /// path with a pool limited to one connection.
    pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError> {
        init_sqlite_vec();
        let mut opt = ConnectOptions::new("sqlite::memory:");
        opt.max_connections(1);
        let db = Database::connect(opt)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Migrator::up(&db, None)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

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

        let now = Utc::now().to_rfc3339();
        let ended_str = ended_at.to_rfc3339();

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
                        created_at: Set(now.clone()),
                        ended_at: Set(ended_str),
                        ..Default::default()
                    };

                    let res = new_summary
                        .insert(txn)
                        .await
                        .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
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
                                .await
                                .map_err(|e| {
                                    MemoryError::MemoryStoreConnectionError(e.to_string())
                                })?;
                        } else {
                            let new_fact = entities::conversation_keyfacts::ActiveModel {
                                card_name: Set(card_name.clone()),
                                summary_id: Set(Some(summary_id)),
                                key: Set(fact.key),
                                value: Set(fact.value),
                                created_at: Set(now.clone()),
                                ..Default::default()
                            };
                            new_fact.insert(txn).await.map_err(|e| {
                                MemoryError::MemoryStoreConnectionError(e.to_string())
                            })?;
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
            created_at: String,
            ended_at: String,
            similarity: f64,
        }

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = Expr::cust_with_values(
            "1.0 - vec_distance_cosine(embedding, ?)",
            vec![query_bytes.clone()],
        );

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
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|row| RecalledSummary {
                entry: ConversationSummary {
                    id: row.id,
                    session_id: row.session_id,
                    card_name: row.card_name,
                    summary: row.summary,
                    embedding: bytes_to_embedding(&row.embedding),
                    created_at: parse_dt(&row.created_at),
                    ended_at: parse_dt(&row.ended_at),
                },
                similarity: row.similarity as f32,
            })
            .collect())
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
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| ConversationSummary {
                id: row.id,
                session_id: row.session_id,
                card_name: row.card_name,
                summary: row.summary,
                embedding: bytes_to_embedding(&row.embedding),
                created_at: parse_dt(&row.created_at),
                ended_at: parse_dt(&row.ended_at),
            })
            .collect())
    }

    /// Counts the number of summaries for a card.
    pub async fn count_summaries(&self, card_name: &str) -> Result<i64, MemoryError> {
        let count = entities::conversation_summaries::Entity::find()
            .filter(entities::conversation_summaries::Column::CardName.eq(card_name))
            .count(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
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
                    .await
                    .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

                let res = entities::conversation_summaries::Entity::delete_by_id(id)
                    .exec(txn)
                    .await
                    .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

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
        let stmt = sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT key, value FROM (
                SELECT key, value, ROW_NUMBER() OVER (PARTITION BY key ORDER BY created_at DESC) as rn
                FROM conversation_keyfacts
                WHERE card_name = ?
            ) WHERE rn = 1
            ORDER BY key ASC",
            vec![card_name.into()],
        );

        let rows = self
            .db
            .query_all(stmt)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        rows.into_iter()
            .map(|row| {
                let key: String = row
                    .try_get("", "key")
                    .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
                let value: String = row
                    .try_get("", "value")
                    .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
                Ok(KeyFact { key, value })
            })
            .collect()
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

        let now = Utc::now().to_rfc3339();
        let new_fact = entities::conversation_keyfacts::ActiveModel {
            card_name: Set(card_name.to_string()),
            summary_id: Set(Some(0)),
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            created_at: Set(now),
            ..Default::default()
        };

        new_fact
            .insert(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Ok(())
    }

    /// Deletes all entries for a specific keyfact key.
    pub async fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, MemoryError> {
        let res = entities::conversation_keyfacts::Entity::delete_many()
            .filter(entities::conversation_keyfacts::Column::CardName.eq(card_name))
            .filter(entities::conversation_keyfacts::Column::Key.eq(key))
            .exec(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
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
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
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

        let now = Utc::now().to_rfc3339();
        let new_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set(role.to_string()),
            content: Set(content.to_string()),
            created_at: Set(now),
            ..Default::default()
        };

        let res = new_log
            .insert(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Ok(res.id)
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
                tracing::error!("[Memory] Failed to save {} log: {}", role, e);
            }
        });
    }

    /// Returns all conversation logs for a session.
    pub async fn get_logs_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String, String)>, MemoryError> {
        let rows = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::SessionId.eq(session_id))
            .order_by_asc(entities::conversation_logs::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

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

        let now = Utc::now().to_rfc3339();
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
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        Ok(())
    }

    /// Lists all stored tool embeddings, one row per (`tool_name`, field, `field_key`, `model_name`).
    pub async fn list_tool_embedding_fields(
        &self,
    ) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError> {
        let rows = entities::tool_embedding_index::Entity::find()
            .all(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

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

    /// Deletes all field embeddings for a tool (cascades across all fields).
    pub async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError> {
        let res = entities::tool_embedding_index::Entity::delete_many()
            .filter(entities::tool_embedding_index::Column::ToolName.eq(tool_name))
            .exec(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
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

        let rows = select
            .into_model::<SearchToolRow>()
            .all(&self.db)
            .await
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

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
        let summaries = self
            .search_summaries(query_embedding, card_name, limit, similarity_threshold)
            .await?;
        let key_facts = self.get_all_keyfacts(card_name).await.unwrap_or_default();
        Ok((summaries, key_facts))
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
        let now = Utc::now().to_rfc3339();

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

        let now = Utc::now().to_rfc3339();
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

        let now = Utc::now().to_rfc3339();
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
}
