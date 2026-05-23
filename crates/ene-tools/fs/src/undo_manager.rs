use chrono::Utc;
use dashmap::DashMap;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rusqlite::Connection;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

/// 1回のツール実行に対するアンドゥ情報
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub id: Uuid,
    pub timestamp: chrono::DateTime<Utc>,
    pub tool_name: String,
    pub operations: Vec<UndoOperation>,
}

impl UndoEntry {
    pub fn new(tool_name: &str, operations: Vec<UndoOperation>) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            tool_name: tool_name.to_string(),
            operations,
        }
    }

    pub fn restore_file(path: PathBuf, original_content: Option<Vec<u8>>) -> UndoOperation {
        UndoOperation::RestoreFile {
            path,
            original_content,
        }
    }

    pub fn delete_created_file(path: PathBuf) -> UndoOperation {
        UndoOperation::DeleteCreatedFile { path }
    }
}

/// 具体的な元に戻し操作
#[derive(Debug, Clone)]
pub enum UndoOperation {
    /// ファイルを元の内容に戻す
    RestoreFile {
        path: PathBuf,
        original_content: Option<Vec<u8>>,
    },
    /// 作成されたファイルを削除
    DeleteCreatedFile { path: PathBuf },
}

/// セッション単位の Undo スタック（インメモリ + SQLite 永続化）
pub struct UndoManager {
    stacks: DashMap<String, VecDeque<UndoEntry>>,
    db: Mutex<Option<Connection>>,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoManager {
    pub fn new() -> Self {
        Self {
            stacks: DashMap::new(),
            db: Mutex::new(None),
        }
    }

    /// DB パスを設定してテーブルを作成する
    pub fn set_db_path(&self, path: &Path) -> Result<(), String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open undo DB: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS undo_entries (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 tool_name TEXT NOT NULL,
                 timestamp TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS undo_operations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 entry_id TEXT NOT NULL REFERENCES undo_entries(id) ON DELETE CASCADE,
                 op_type TEXT NOT NULL,
                 path TEXT NOT NULL,
                 original_content BLOB,
                 sort_order INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_undo_entries_session
                 ON undo_entries(session_id);",
        )
        .map_err(|e| format!("Failed to create undo tables: {e}"))?;
        *self.db.lock().unwrap() = Some(conn);
        Ok(())
    }

    /// DB から特定セッションのエントリを読み込んでインメモリスタックに復元する
    fn load_session_from_db(&self, session_id: &str) -> Result<(), String> {
        let guard = self.db.lock().unwrap();
        let db = guard.as_ref().ok_or("Undo DB not initialized")?;

        let mut stmt = db
            .prepare(
                "SELECT id, tool_name, timestamp FROM undo_entries
                 WHERE session_id = ?1 ORDER BY rowid",
            )
            .map_err(|e| format!("Failed to prepare: {e}"))?;

        let entries: Vec<(String, String, String)> = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("Failed to query: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        if entries.is_empty() {
            return Ok(());
        }

        let mut ops_stmt = db
            .prepare(
                "SELECT op_type, path, original_content, sort_order FROM undo_operations
                 WHERE entry_id = ?1 ORDER BY sort_order",
            )
            .map_err(|e| format!("Failed to prepare ops: {e}"))?;

        let mut stack = VecDeque::new();
        for (id_str, tool_name, ts_str) in &entries {
            let id = Uuid::parse_str(id_str).unwrap_or_default();
            let timestamp = ts_str.parse::<chrono::DateTime<Utc>>().unwrap_or_default();

            let ops: Vec<UndoOperation> = ops_stmt
                .query_map([id_str], |row| {
                    let op_type: String = row.get(0)?;
                    let path_str: String = row.get(1)?;
                    let blob: Option<Vec<u8>> = row.get(2)?;
                    let _sort_order: i32 = row.get(3)?;
                    let path = PathBuf::from(path_str);
                    Ok((op_type, path, blob))
                })
                .map_err(|e| format!("Failed to query ops: {e}"))?
                .filter_map(|r| r.ok())
                .map(|(op_type, path, blob)| match op_type.as_str() {
                    "RestoreFile" => {
                        let content = blob.map(|b| decompress(&b));
                        UndoOperation::RestoreFile {
                            path,
                            original_content: content,
                        }
                    }
                    _ => UndoOperation::DeleteCreatedFile { path },
                })
                .collect();

            stack.push_back(UndoEntry {
                id,
                timestamp,
                tool_name: tool_name.clone(),
                operations: ops,
            });
        }

        self.stacks.insert(session_id.to_string(), stack);
        Ok(())
    }

    /// ファイル復元操作を記録
    pub fn push_restore_file(
        &self,
        session_id: &str,
        tool_name: &str,
        path: PathBuf,
        original_content: Option<Vec<u8>>,
    ) {
        self.push(
            session_id,
            UndoEntry::new(
                tool_name,
                vec![UndoEntry::restore_file(path, original_content)],
            ),
        );
    }

    /// 作成ファイル削除操作を記録
    pub fn push_delete_created_file(&self, session_id: &str, tool_name: &str, path: PathBuf) {
        self.push(
            session_id,
            UndoEntry::new(tool_name, vec![UndoEntry::delete_created_file(path)]),
        );
    }

    /// 操作を記録
    pub fn push(&self, session_id: &str, entry: UndoEntry) {
        let mut stack = self.stacks.entry(session_id.to_string()).or_default();
        stack.push_back(entry.clone());

        if let Some(ref conn) = *self.db.lock().unwrap() {
            if let Err(e) = self.persist_entry(conn, session_id, &entry) {
                eprintln!("[UndoManager] Failed to persist undo entry: {e}");
            }
        }
    }

    fn persist_entry(
        &self,
        conn: &Connection,
        session_id: &str,
        entry: &UndoEntry,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO undo_entries (id, session_id, tool_name, timestamp) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                entry.id.to_string(),
                session_id,
                entry.tool_name,
                entry.timestamp.to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to insert undo entry: {e}"))?;

        for (i, op) in entry.operations.iter().enumerate() {
            match op {
                UndoOperation::RestoreFile {
                    path,
                    original_content,
                } => {
                    let compressed = original_content.as_ref().map(|c| compress(c));
                    conn.execute(
                        "INSERT INTO undo_operations (entry_id, op_type, path, original_content, sort_order)
                         VALUES (?1, 'RestoreFile', ?2, ?3, ?4)",
                        rusqlite::params![entry.id.to_string(), path.to_string_lossy().as_ref(), compressed, i as i32],
                    )
                    .map_err(|e| format!("Failed to insert undo op: {e}"))?;
                }
                UndoOperation::DeleteCreatedFile { path } => {
                    conn.execute(
                        "INSERT INTO undo_operations (entry_id, op_type, path, original_content, sort_order)
                         VALUES (?1, 'DeleteCreatedFile', ?2, NULL, ?3)",
                        rusqlite::params![entry.id.to_string(), path.to_string_lossy().as_ref(), i as i32],
                    )
                    .map_err(|e| format!("Failed to insert undo op: {e}"))?;
                }
            }
        }
        Ok(())
    }

    /// 最新の操作を元に戻す
    pub async fn undo(&self, session_id: &str) -> Result<Vec<String>, String> {
        let has_stack = self.stacks.contains_key(session_id);
        if !has_stack && self.db.lock().unwrap().is_some() {
            if let Err(e) = self.load_session_from_db(session_id) {
                eprintln!("[UndoManager] Failed to load from DB: {e}");
            }
        }

        let mut stack = self
            .stacks
            .get_mut(session_id)
            .ok_or("No undo history for this session")?;
        let entry = stack.pop_back().ok_or("Undo stack is empty")?;

        let id_str = entry.id.to_string();

        let mut logs = Vec::new();
        logs.push(format!("Undoing {} ({})", entry.tool_name, entry.id));

        for op in &entry.operations {
            match op {
                UndoOperation::RestoreFile {
                    path,
                    original_content,
                } => match original_content {
                    Some(content) => {
                        tokio::fs::write(path, content).await.map_err(|e| {
                            format!("Failed to restore file {}: {}", path.display(), e)
                        })?;
                        logs.push(format!("Restored file: {}", path.display()));
                    }
                    None => {
                        if path.exists() {
                            tokio::fs::remove_file(path).await.map_err(|e| {
                                format!("Failed to remove created file {}: {}", path.display(), e)
                            })?;
                            logs.push(format!("Removed created file: {}", path.display()));
                        }
                    }
                },
                UndoOperation::DeleteCreatedFile { path } => {
                    if path.exists() {
                        if path.is_file() {
                            tokio::fs::remove_file(path).await.map_err(|e| {
                                format!("Failed to delete created file {}: {}", path.display(), e)
                            })?;
                        } else if path.is_dir() {
                            tokio::fs::remove_dir_all(path).await.map_err(|e| {
                                format!(
                                    "Failed to delete created directory {}: {}",
                                    path.display(),
                                    e
                                )
                            })?;
                        }
                        logs.push(format!("Deleted created path: {}", path.display()));
                    }
                }
            }
        }

        if let Some(ref conn) = *self.db.lock().unwrap() {
            if let Err(e) = conn.execute(
                "DELETE FROM undo_operations WHERE entry_id = ?1",
                rusqlite::params![id_str],
            ) {
                eprintln!("[UndoManager] Failed to delete undo ops from DB: {e}");
            }
            if let Err(e) = conn.execute(
                "DELETE FROM undo_entries WHERE id = ?1",
                rusqlite::params![id_str],
            ) {
                eprintln!("[UndoManager] Failed to delete undo entry from DB: {e}");
            }
        }

        if stack.is_empty() {
            drop(stack);
            self.stacks.remove(session_id);
        }

        Ok(logs)
    }

    /// スタックをクリア
    pub fn clear(&self, session_id: &str) {
        self.stacks.remove(session_id);

        if let Some(ref conn) = *self.db.lock().unwrap() {
            let _ = conn.execute(
                "DELETE FROM undo_operations WHERE entry_id IN (SELECT id FROM undo_entries WHERE session_id = ?1)",
                rusqlite::params![session_id],
            );
            let _ = conn.execute(
                "DELETE FROM undo_entries WHERE session_id = ?1",
                rusqlite::params![session_id],
            );
        }
    }

    /// スタックの長さを取得
    pub fn len(&self, session_id: &str) -> usize {
        let has = self.stacks.contains_key(session_id);
        if !has && self.db.lock().unwrap().is_some() {
            if let Ok(()) = self.load_session_from_db(session_id) {
                // fall through to check again
            }
        }
        self.stacks.get(session_id).map(|s| s.len()).unwrap_or(0)
    }

    /// 空かどうか
    pub fn is_empty(&self, session_id: &str) -> bool {
        self.len(session_id) == 0
    }
}

/// zlib 圧縮
fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_else(|_| data.to_vec())
}

/// zlib 展開
fn decompress(data: &[u8]) -> Vec<u8> {
    let mut decoder = ZlibDecoder::new(data);
    let mut out = Vec::new();
    let _ = decoder.read_to_end(&mut out);
    if out.is_empty() {
        return data.to_vec();
    }
    out
}

/// 破壊的操作前にバックアップを取得
pub async fn backup_file(path: &Path) -> Option<Vec<u8>> {
    if path.exists() && path.is_file() {
        tokio::fs::read(path).await.ok()
    } else {
        None
    }
}
