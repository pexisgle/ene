use crate::error::AiCoreError;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

mod schema;
mod serialization;
use serialization::{bytes_to_f32_vec, f32_slice_to_bytes};

/// キーバリュー型の事実
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KeyFact {
    pub key: String,
    pub value: String,
}

/// 会話要約エントリ
#[derive(Debug, Clone)]
pub struct ConversationSummary {
    pub id: i64,
    pub session_id: String,
    pub card_name: String,
    pub summary: String,
    pub embedding: Vec<f32>,
    pub created_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

/// ベクトル検索で呼び出された要約（スコア付き）
#[derive(Debug, Clone)]
pub struct RecalledSummary {
    pub entry: ConversationSummary,
    pub similarity: f32,
}

/// SQLite ベースの長期記憶ストア
pub struct MemoryStore {
    conn: Mutex<Connection>,
    pub embedding_dim: usize,
}

// Safety: `rusqlite::Connection` is `!Send`, but we always access it through
// `std::sync::Mutex`, ensuring only one thread uses it at a time.
// This is a common and safe pattern for sharing a SQLite connection across threads.
unsafe impl Send for MemoryStore {}
unsafe impl Sync for MemoryStore {}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

impl MemoryStore {
    fn init(conn: Connection, embedding_dim: usize) -> Result<Self, AiCoreError> {
        schema::init_sqlite_vec();
        let store = Self {
            conn: Mutex::new(conn),
            embedding_dim,
        };
        schema::initialize_schema(&store.conn.lock().unwrap())?;
        Ok(store)
    }

    pub fn open(path: &Path, embedding_dim: usize) -> Result<Self, AiCoreError> {
        Self::init(Connection::open(path)?, embedding_dim)
    }

    pub fn open_in_memory(embedding_dim: usize) -> Result<Self, AiCoreError> {
        Self::init(Connection::open_in_memory()?, embedding_dim)
    }

    // ── Conversation Summaries ────────────────────────────────────────────────

    /// 会話要約を保存する（keyfactsも同時に保存）
    ///
    /// keyfacts の処理ルール:
    /// - value が空文字 `""` の場合 → その key の既存事実を削除
    /// - value が通常値の場合 → 新規挿入（get_all_keyfacts で最新値が自動選択される）
    pub fn insert_summary(
        &self,
        session_id: &str,
        card_name: &str,
        summary: &str,
        key_facts: &[KeyFact],
        embedding: &[f32],
        ended_at: DateTime<Utc>,
    ) -> Result<i64, AiCoreError> {
        let blob = f32_slice_to_bytes(embedding);
        let now = Utc::now().to_rfc3339();
        let ended_str = ended_at.to_rfc3339();

        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO conversation_summaries
             (session_id, card_name, summary, embedding, created_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, card_name, summary, blob, now, ended_str],
        )?;

        let summary_id = tx.last_insert_rowid();

        for fact in key_facts {
            if fact.value.is_empty() {
                tx.execute(
                    "DELETE FROM conversation_keyfacts WHERE card_name = ?1 AND key = ?2",
                    params![card_name, fact.key],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO conversation_keyfacts (card_name, summary_id, key, value, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![card_name, summary_id, fact.key, fact.value, now],
                )?;
            }
        }

        tx.commit()?;
        Ok(summary_id)
    }

    /// 要約をベクトル検索する（sqlite-vec 使用）
    pub fn search_summaries(
        &self,
        query_embedding: &[f32],
        card_name: &str,
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<RecalledSummary>, AiCoreError> {
        let query_blob = f32_slice_to_bytes(query_embedding);
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, session_id, card_name, summary, embedding, created_at, ended_at,
                    1.0 - vec_distance_cosine(embedding, ?2) AS similarity
             FROM conversation_summaries
             WHERE card_name = ?1
               AND (1.0 - vec_distance_cosine(embedding, ?2)) >= ?3
             ORDER BY similarity DESC
             LIMIT ?4",
        )?;

        let rows = stmt.query_map(
            params![card_name, query_blob, similarity_threshold, limit as i64],
            |row| {
                let blob: Vec<u8> = row.get(4)?;
                let created_str: String = row.get(5)?;
                let ended_str: String = row.get(6)?;
                let similarity: f64 = row.get(7)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    blob,
                    created_str,
                    ended_str,
                    similarity as f32,
                ))
            },
        )?;

        let mut results = Vec::new();
        for row_result in rows {
            let (id, sid, cn, summary, blob, ca, ea, sim) =
                row_result.map_err(AiCoreError::MemoryStoreError)?;

            let embedding = bytes_to_f32_vec(&blob);

            results.push(RecalledSummary {
                entry: ConversationSummary {
                    id,
                    session_id: sid,
                    card_name: cn,
                    summary,
                    embedding,
                    created_at: parse_dt(&ca),
                    ended_at: parse_dt(&ea),
                },
                similarity: sim,
            });
        }

        Ok(results)
    }

    /// 直近の要約を取得する
    pub fn list_recent_summaries(
        &self,
        card_name: &str,
        limit: usize,
    ) -> Result<Vec<ConversationSummary>, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, card_name, summary, embedding, created_at, ended_at
             FROM conversation_summaries
             WHERE card_name = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![card_name, limit as i64], |row| {
            let blob: Vec<u8> = row.get(4)?;
            let created_str: String = row.get(5)?;
            let ended_str: String = row.get(6)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                blob,
                created_str,
                ended_str,
            ))
        })?;

        let mut entries = Vec::new();
        for r in rows {
            let (id, sid, cn, summary, blob, ca, ea) = r.map_err(AiCoreError::MemoryStoreError)?;
            entries.push(ConversationSummary {
                id,
                session_id: sid,
                card_name: cn,
                summary,
                embedding: bytes_to_f32_vec(&blob),
                created_at: parse_dt(&ca),
                ended_at: parse_dt(&ea),
            });
        }

        Ok(entries)
    }

    /// 要約数を返す
    pub fn count_summaries(&self, card_name: &str) -> Result<i64, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM conversation_summaries WHERE card_name = ?1",
            params![card_name],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// 要約を削除する（関連keyfactsも削除）
    pub fn delete_summary(&self, id: i64) -> Result<usize, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM conversation_keyfacts WHERE summary_id = ?1",
            params![id],
        )?;
        let count = tx.execute(
            "DELETE FROM conversation_summaries WHERE id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(count)
    }

    // ── Key Facts ─────────────────────────────────────────────────────────────

    /// キャラクターの全keyfactsを取得（keyでソート、重複は最新のvalue）
    pub fn get_all_keyfacts(&self, card_name: &str) -> Result<Vec<KeyFact>, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key, value FROM (
                SELECT key, value, ROW_NUMBER() OVER (PARTITION BY key ORDER BY created_at DESC) as rn
                FROM conversation_keyfacts
                WHERE card_name = ?1
            ) WHERE rn = 1
            ORDER BY key ASC",
        )?;

        let rows = stmt.query_map(params![card_name], |row| {
            Ok(KeyFact {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })?;

        let mut facts = Vec::new();
        for r in rows {
            facts.push(r.map_err(AiCoreError::MemoryStoreError)?);
        }
        Ok(facts)
    }

    /// 特定のkeyfactを更新する（UPSERT）
    pub fn upsert_keyfact(
        &self,
        card_name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), AiCoreError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_keyfacts (card_name, summary_id, key, value, created_at)
             VALUES (?1, 0, ?2, ?3, ?4)",
            params![card_name, key, value, now],
        )?;
        Ok(())
    }

    /// 特定のkeyfactを削除する
    pub fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "DELETE FROM conversation_keyfacts WHERE card_name = ?1 AND key = ?2",
            params![card_name, key],
        )?;
        Ok(count)
    }

    /// keyfacts数を返す
    pub fn count_keyfacts(&self, card_name: &str) -> Result<i64, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT key) FROM conversation_keyfacts WHERE card_name = ?1",
            params![card_name],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    // ── Conversation Logs ─────────────────────────────────────────────────────

    pub fn insert_log(
        &self,
        session_id: &str,
        card_name: &str,
        role: &str,
        content: &str,
    ) -> Result<i64, AiCoreError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_logs (session_id, card_name, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, card_name, role, content, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_logs_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String, String)>, AiCoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT role, content, created_at FROM conversation_logs
             WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r.map_err(AiCoreError::MemoryStoreError)?);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::serialization::{bytes_to_f32_vec, f32_slice_to_bytes};
    use super::*;

    #[test]
    fn test_roundtrip_bytes() {
        let original = vec![1.0_f32, 0.5, -0.25, 0.0];
        let bytes = f32_slice_to_bytes(&original);
        let restored = bytes_to_f32_vec(&bytes);
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
        let conn = store.conn.lock().unwrap();
        for f in &facts {
            conn.execute(
                "INSERT INTO conversation_keyfacts (card_name, summary_id, key, value, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["char", summary_id, f.key, f.value, now],
            )
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
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_keyfacts (card_name, summary_id, key, value, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["char", summary_id, "job", "engineer", now],
        )
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
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_keyfacts (card_name, summary_id, key, value, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["char", summary_id, "job", "engineer", now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversation_keyfacts (card_name, summary_id, key, value, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["char", summary_id, "hobby", "guitar", now],
        )
        .unwrap();
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
