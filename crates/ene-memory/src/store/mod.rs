use crate::error::MemoryError;
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use ene_config::serde;
use std::path::Path;

mod models;
use models::{
    EmbeddingBlob, NewConversationLog, NewConversationSummary, NewKeyFact, NewToolEmbedding,
};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Registers the sqlite-vec extension and runs pending Diesel migrations.
pub fn init_sqlite_vec(conn: &mut SqliteConnection) -> Result<(), MemoryError> {
    use rusqlite::ffi::sqlite3_auto_extension;
    use sqlite_vec::sqlite3_vec_init;
    // SAFETY: sqlite3_auto_extension expects a function pointer cast to a void pointer.
    // sqlite3_vec_init is a C function with the correct signature (extern "C" fn()),
    // and transmuting it to *const () is a well-known pattern for registering SQLite
    // extensions. The function pointer remains valid for the lifetime of the process.
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
    Ok(())
}

/// A key-value fact about the user.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(crate = "ene_config::serde")]
pub struct KeyFact {
    /// The key identifier for this fact.
    pub key: String,
    /// The value associated with the key.
    pub value: String,
}

/// A stored conversation summary entry with its embedding vector.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(crate = "ene_config::serde")]
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

type SqlitePool = diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<SqliteConnection>>;

/// SQLite-backed long-term memory store with vector similarity search.
///
/// Uses `r2d2` connection pooling and `sqlite-vec` for cosine-similarity queries.
/// Manages four tables: `conversation_summaries`, `conversation_keyfacts`,
/// `conversation_logs`, and `tool_embeddings`.
pub struct MemoryStore {
    pool: SqlitePool,
    /// Dimensionality of the embedding vectors.
    pub embedding_dim: usize,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

impl MemoryStore {
    fn init(pool: SqlitePool, embedding_dim: usize) -> Result<Self, MemoryError> {
        let mut conn = pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        init_sqlite_vec(&mut conn)?;
        Ok(Self {
            pool,
            embedding_dim,
        })
    }

    /// Opens a persistent memory store at the given file path.
    ///
    /// Creates the database file if it doesn't exist. Runs embedded Diesel migrations
    /// and registers the `sqlite-vec` extension automatically.
    pub fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| MemoryError::MemoryStoreConnectionError("Invalid path".to_string()))?;
        let manager = diesel::r2d2::ConnectionManager::<SqliteConnection>::new(path_str);
        let pool = diesel::r2d2::Pool::builder()
            .build(manager)
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        Self::init(pool, embedding_dim)
    }

    /// Opens an in-memory memory store (useful for testing).
    ///
    /// Uses `":memory:"` as the database path with a pool limited to one connection.
    pub fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError> {
        let manager = diesel::r2d2::ConnectionManager::<SqliteConnection>::new(":memory:");
        let pool = diesel::r2d2::Pool::builder()
            .max_size(1)
            .build(manager)
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        Self::init(pool, embedding_dim)
    }

    // ── Conversation Summaries ────────────────────────────────────────────────

    /// Inserts a conversation summary and associated key facts in a single transaction.
    ///
    /// Facts with an empty `value` field are treated as deletions for that key.
    /// Returns the new summary's auto-increment ID.
    pub fn insert_summary(
        &self,
        session_id: &str,
        card_name: &str,
        summary: &str,
        key_facts: &[KeyFact],
        embedding: &[f32],
        ended_at: DateTime<Utc>,
    ) -> Result<i64, MemoryError> {
        let now = Utc::now().to_rfc3339();
        let ended_str = ended_at.to_rfc3339();

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        conn.transaction(|conn| {
            let new_summary = NewConversationSummary {
                session_id,
                card_name,
                summary,
                embedding: EmbeddingBlob(embedding.to_vec()),
                created_at: &now,
                ended_at: &ended_str,
            };

            diesel::insert_into(crate::schema::conversation_summaries::table)
                .values(&new_summary)
                .execute(conn)?;

            let summary_id: i64 = diesel::select(diesel::dsl::sql::<diesel::sql_types::BigInt>(
                "last_insert_rowid()",
            ))
            .get_result(conn)?;

            for fact in key_facts {
                if fact.value.is_empty() {
                    diesel::delete(
                        crate::schema::conversation_keyfacts::table
                            .filter(crate::schema::conversation_keyfacts::card_name.eq(card_name))
                            .filter(crate::schema::conversation_keyfacts::key.eq(&fact.key)),
                    )
                    .execute(conn)?;
                } else {
                    let new_fact = NewKeyFact {
                        card_name,
                        summary_id: Some(summary_id),
                        key: &fact.key,
                        value: &fact.value,
                        created_at: &now,
                    };
                    diesel::insert_into(crate::schema::conversation_keyfacts::table)
                        .values(&new_fact)
                        .execute(conn)?;
                }
            }

            Ok(summary_id)
        })
    }

    /// Searches summaries by cosine similarity to the query embedding.
    ///
    /// Uses `vec_distance_cosine` for fast approximate matching.
    /// Results are filtered by `card_name` and `similarity_threshold`.
    pub fn search_summaries(
        &self,
        query_embedding: &[f32],
        card_name: &str,
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<RecalledSummary>, MemoryError> {
        let query_blob = EmbeddingBlob(query_embedding.to_vec());
        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        let query = "SELECT id, session_id, card_name, summary, embedding, created_at, ended_at,
                            1.0 - vec_distance_cosine(embedding, ?) AS similarity
                     FROM conversation_summaries
                     WHERE card_name = ?
                       AND (1.0 - vec_distance_cosine(embedding, ?)) >= ?
                     ORDER BY similarity DESC
                     LIMIT ?";

        let results = diesel::sql_query(query)
            .bind::<diesel::sql_types::Binary, _>(&query_blob)
            .bind::<diesel::sql_types::Text, _>(card_name)
            .bind::<diesel::sql_types::Binary, _>(&query_blob)
            .bind::<diesel::sql_types::Float, _>(similarity_threshold)
            .bind::<diesel::sql_types::BigInt, _>(limit as i64)
            .load::<SearchSummaryRow>(&mut *conn)?;

        results
            .into_iter()
            .map(|row| {
                Ok(RecalledSummary {
                    entry: ConversationSummary {
                        id: row.id,
                        session_id: row.session_id,
                        card_name: row.card_name,
                        summary: row.summary,
                        embedding: row.embedding.0,
                        created_at: parse_dt(&row.created_at),
                        ended_at: parse_dt(&row.ended_at),
                    },
                    similarity: row.similarity,
                })
            })
            .collect()
    }

    /// Lists the most recent conversation summaries for a card.
    pub fn list_recent_summaries(
        &self,
        card_name: &str,
        limit: usize,
    ) -> Result<Vec<ConversationSummary>, MemoryError> {
        use crate::schema::conversation_summaries::dsl;

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let rows = dsl::conversation_summaries
            .filter(dsl::card_name.eq(card_name))
            .order(dsl::created_at.desc())
            .limit(limit as i64)
            .select(models::ConversationSummaryRow::as_select())
            .load(&mut *conn)?;

        rows.into_iter()
            .map(|row| {
                Ok(ConversationSummary {
                    id: row.id,
                    session_id: row.session_id,
                    card_name: row.card_name,
                    summary: row.summary,
                    embedding: row.embedding.0,
                    created_at: parse_dt(&row.created_at),
                    ended_at: parse_dt(&row.ended_at),
                })
            })
            .collect()
    }

    /// Counts the number of summaries for a card.
    pub fn count_summaries(&self, card_name: &str) -> Result<i64, MemoryError> {
        use crate::schema::conversation_summaries::dsl;

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let count = dsl::conversation_summaries
            .filter(dsl::card_name.eq(card_name))
            .count()
            .get_result(&mut *conn)?;
        Ok(count)
    }

    /// Deletes a summary and its associated keyfacts.
    pub fn delete_summary(&self, id: i64) -> Result<usize, MemoryError> {
        use crate::schema::{conversation_keyfacts, conversation_summaries};

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        conn.transaction(|conn| {
            diesel::delete(
                conversation_keyfacts::table.filter(conversation_keyfacts::summary_id.eq(id)),
            )
            .execute(conn)?;

            let count = diesel::delete(
                conversation_summaries::table.filter(conversation_summaries::id.eq(id)),
            )
            .execute(conn)?;

            Ok(count)
        })
    }

    // ── Key Facts ─────────────────────────────────────────────────────────────

    /// Returns all unique keyfacts for a card, with the latest value per key.
    pub fn get_all_keyfacts(&self, card_name: &str) -> Result<Vec<KeyFact>, MemoryError> {
        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        let query = "SELECT key, value FROM (
            SELECT key, value, ROW_NUMBER() OVER (PARTITION BY key ORDER BY created_at DESC) as rn
            FROM conversation_keyfacts
            WHERE card_name = ?
        ) WHERE rn = 1
        ORDER BY key ASC";

        let rows = diesel::sql_query(query)
            .bind::<diesel::sql_types::Text, _>(card_name)
            .load::<KeyFactQueryResult>(&mut *conn)?;

        Ok(rows
            .into_iter()
            .map(|row| KeyFact {
                key: row.key,
                value: row.value,
            })
            .collect())
    }

    /// Inserts or updates a keyfact for a card.
    pub fn upsert_keyfact(
        &self,
        card_name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), MemoryError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        let new_fact = NewKeyFact {
            card_name,
            summary_id: Some(0),
            key,
            value,
            created_at: &now,
        };

        diesel::insert_into(crate::schema::conversation_keyfacts::table)
            .values(&new_fact)
            .execute(&mut *conn)?;

        Ok(())
    }

    /// Deletes all entries for a specific keyfact key.
    pub fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, MemoryError> {
        use crate::schema::conversation_keyfacts::dsl;

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let count = diesel::delete(
            dsl::conversation_keyfacts
                .filter(dsl::card_name.eq(card_name))
                .filter(dsl::key.eq(key)),
        )
        .execute(&mut *conn)?;
        Ok(count)
    }

    /// Counts the number of distinct keyfacts for a card.
    pub fn count_keyfacts(&self, card_name: &str) -> Result<i64, MemoryError> {
        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        let query =
            "SELECT COUNT(DISTINCT key) as count FROM conversation_keyfacts WHERE card_name = ?";
        let result: CountResult = diesel::sql_query(query)
            .bind::<diesel::sql_types::Text, _>(card_name)
            .get_result(&mut *conn)?;

        Ok(result.count)
    }

    // ── Conversation Logs ─────────────────────────────────────────────────────

    /// Inserts a conversation log entry.
    pub fn insert_log(
        &self,
        session_id: &str,
        card_name: &str,
        role: &str,
        content: &str,
    ) -> Result<i64, MemoryError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        let new_log = NewConversationLog {
            session_id,
            card_name,
            role,
            content,
            created_at: &now,
        };

        diesel::insert_into(crate::schema::conversation_logs::table)
            .values(&new_log)
            .execute(&mut *conn)?;

        let id: i64 = diesel::select(diesel::dsl::sql::<diesel::sql_types::BigInt>(
            "last_insert_rowid()",
        ))
        .get_result(&mut *conn)?;

        Ok(id)
    }

    /// Returns all conversation logs for a session.
    pub fn get_logs_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String, String)>, MemoryError> {
        use crate::schema::conversation_logs::dsl;

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let rows = dsl::conversation_logs
            .filter(dsl::session_id.eq(session_id))
            .order(dsl::created_at.asc())
            .select(models::ConversationLogRow::as_select())
            .load(&mut *conn)?;

        Ok(rows
            .into_iter()
            .map(|row| (row.role, row.content, row.created_at))
            .collect())
    }

    // ── Tool Embeddings ─────────────────────────────────────────────────────

    /// Inserts or updates a tool embedding in the database.
    pub fn upsert_tool_embedding(
        &self,
        tool_name: &str,
        version_hash: &str,
        embedding: &[f32],
    ) -> Result<(), MemoryError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;

        let new_embedding = NewToolEmbedding {
            tool_name,
            version_hash,
            embedding: EmbeddingBlob(embedding.to_vec()),
            created_at: &now,
        };

        diesel::insert_into(crate::schema::tool_embeddings::table)
            .values(&new_embedding)
            .on_conflict(crate::schema::tool_embeddings::tool_name)
            .do_update()
            .set((
                crate::schema::tool_embeddings::version_hash.eq(&new_embedding.version_hash),
                crate::schema::tool_embeddings::embedding.eq(&new_embedding.embedding),
                crate::schema::tool_embeddings::created_at.eq(&new_embedding.created_at),
            ))
            .execute(&mut *conn)?;

        Ok(())
    }

    /// Lists all stored tool embeddings.
    pub fn list_tool_embeddings(&self) -> Result<Vec<(String, String, Vec<f32>)>, MemoryError> {
        use crate::schema::tool_embeddings::dsl;

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let rows = dsl::tool_embeddings
            .select(models::ToolEmbeddingRow::as_select())
            .load(&mut *conn)?;

        Ok(rows
            .into_iter()
            .map(|row| (row.tool_name, row.version_hash, row.embedding.0))
            .collect())
    }

    /// Deletes a tool embedding from the database.
    pub fn delete_tool_embedding(&self, tool_name: &str) -> Result<usize, MemoryError> {
        use crate::schema::tool_embeddings::dsl;

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let count = diesel::delete(dsl::tool_embeddings.filter(dsl::tool_name.eq(tool_name)))
            .execute(&mut *conn)?;
        Ok(count)
    }

    /// Searches tool embeddings by cosine similarity to the query.
    pub fn search_tools(
        &self,
        query_embedding: &[f32],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(String, f32)>, MemoryError> {
        let query_blob = EmbeddingBlob(query_embedding.to_vec());

        let mut conn = self
            .pool
            .get()
            .map_err(|e| MemoryError::MemoryStoreConnectionError(e.to_string()))?;
        let query = "SELECT tool_name,
                            1.0 - vec_distance_cosine(embedding, ?) AS similarity
                     FROM tool_embeddings
                     WHERE (1.0 - vec_distance_cosine(embedding, ?)) >= ?
                     ORDER BY similarity DESC
                     LIMIT ?";

        let rows = diesel::sql_query(query)
            .bind::<diesel::sql_types::Binary, _>(&query_blob)
            .bind::<diesel::sql_types::Binary, _>(&query_blob)
            .bind::<diesel::sql_types::Float, _>(similarity_threshold)
            .bind::<diesel::sql_types::BigInt, _>(limit as i64)
            .load::<SearchToolRow>(&mut *conn)?;

        Ok(rows
            .into_iter()
            .map(|row| (row.tool_name, row.similarity))
            .collect())
    }
}

#[derive(diesel::QueryableByName)]
struct SearchSummaryRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    session_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    card_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    summary: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    embedding: EmbeddingBlob,
    #[diesel(sql_type = diesel::sql_types::Text)]
    created_at: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    ended_at: String,
    #[diesel(sql_type = diesel::sql_types::Float)]
    similarity: f32,
}

#[derive(diesel::QueryableByName)]
struct KeyFactQueryResult {
    #[diesel(sql_type = diesel::sql_types::Text)]
    key: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    value: String,
}

#[derive(diesel::QueryableByName)]
struct CountResult {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(diesel::QueryableByName)]
struct SearchToolRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    tool_name: String,
    #[diesel(sql_type = diesel::sql_types::Float)]
    similarity: f32,
}

#[cfg(test)]
mod tests {
    use super::models::EmbeddingBlob;
    use super::*;

    #[test]
    fn test_roundtrip_bytes() {
        let original = vec![1.0_f32, 0.5, -0.25, 0.0];
        let blob = EmbeddingBlob(original.clone());
        let restored = blob.0;
        for (a, b) in original.iter().zip(restored.iter()) {
            assert!((a - b).abs() < 1e-7, "Mismatch: {} != {}", a, b);
        }
    }

    #[test]
    #[ignore = "sqlite-vec extension not available for in-memory DB in test environment"]
    fn test_insert_and_search_summaries() {
        let store = MemoryStore::open_in_memory(4).unwrap();
        let emb_a = vec![1.0_f32, 0.0, 0.0, 0.0];
        let emb_b = vec![0.0_f32, 1.0, 0.0, 0.0];

        store
            .insert_summary("s1", "char", "Summary A", &[], &emb_a, Utc::now())
            .unwrap();
        store
            .insert_summary("s2", "char", "Summary B", &[], &emb_b, Utc::now())
            .unwrap();

        let results = store.search_summaries(&emb_a, "char", 5, 0.5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.summary, "Summary A");
        assert!((results[0].similarity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_keyfacts_crud() {
        let store = MemoryStore::open_in_memory(4).unwrap();

        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let summary_id = store
            .insert_summary("s1", "char", "Summary", &[], &emb, Utc::now())
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
        let mut conn = store.pool.get().unwrap();
        for f in &facts {
            let new_fact = NewKeyFact {
                card_name: "char",
                summary_id: Some(summary_id),
                key: &f.key,
                value: &f.value,
                created_at: &now,
            };
            diesel::insert_into(crate::schema::conversation_keyfacts::table)
                .values(&new_fact)
                .execute(&mut *conn)
                .unwrap();
        }
        drop(conn);

        let all_facts = store.get_all_keyfacts("char").unwrap();
        assert_eq!(all_facts.len(), 2);
        assert_eq!(all_facts[0].key, "food");
        assert_eq!(all_facts[1].key, "job");

        store.upsert_keyfact("char", "food", "sushi").unwrap();
        let all_facts = store.get_all_keyfacts("char").unwrap();
        let food_fact = all_facts.iter().find(|f| f.key == "food").unwrap();
        assert_eq!(food_fact.value, "sushi");

        store.delete_keyfact("char", "food").unwrap();
        let all_facts = store.get_all_keyfacts("char").unwrap();
        assert_eq!(all_facts.len(), 1);
    }

    #[test]
    fn test_delete_summary_cascades() {
        let store = MemoryStore::open_in_memory(4).unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let summary_id = store
            .insert_summary("s1", "char", "Summary", &[], &emb, Utc::now())
            .unwrap();

        let now = Utc::now().to_rfc3339();
        let mut conn = store.pool.get().unwrap();
        let new_fact = NewKeyFact {
            card_name: "char",
            summary_id: Some(summary_id),
            key: "job",
            value: "engineer",
            created_at: &now,
        };
        diesel::insert_into(crate::schema::conversation_keyfacts::table)
            .values(&new_fact)
            .execute(&mut *conn)
            .unwrap();
        drop(conn);

        store.delete_summary(summary_id).unwrap();
        assert_eq!(store.count_summaries("char").unwrap(), 0);
        assert_eq!(store.count_keyfacts("char").unwrap(), 0);
    }

    #[test]
    fn test_insert_summary_with_empty_value_deletes_keyfact() {
        let store = MemoryStore::open_in_memory(4).unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let summary_id = store
            .insert_summary("s1", "char", "Summary", &[], &emb, Utc::now())
            .unwrap();

        let now = Utc::now().to_rfc3339();
        let mut conn = store.pool.get().unwrap();
        for (k, v) in &[("job", "engineer"), ("hobby", "guitar")] {
            let new_fact = NewKeyFact {
                card_name: "char",
                summary_id: Some(summary_id),
                key: k,
                value: v,
                created_at: &now,
            };
            diesel::insert_into(crate::schema::conversation_keyfacts::table)
                .values(&new_fact)
                .execute(&mut *conn)
                .unwrap();
        }
        drop(conn);

        assert_eq!(store.get_all_keyfacts("char").unwrap().len(), 2);

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
                        value: "".to_string(),
                    },
                ],
                &emb2,
                Utc::now(),
            )
            .unwrap();

        let all_facts = store.get_all_keyfacts("char").unwrap();
        assert_eq!(all_facts.len(), 1);
        assert_eq!(all_facts[0].key, "job");
        assert_eq!(all_facts[0].value, "designer");
    }
}
