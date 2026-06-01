use chrono::Utc;
use dashmap::DashMap;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

use crate::utils::schema::{undo_entries, undo_operations};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Undo information for a single tool execution
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

/// Concrete undo operation
#[derive(Debug, Clone)]
pub enum UndoOperation {
    /// Restores a file to its original content
    RestoreFile {
        path: PathBuf,
        original_content: Option<Vec<u8>>,
    },
    /// Deletes a created file
    DeleteCreatedFile { path: PathBuf },
}

/// Per-session undo stack (in-memory + SQLite persistence)
pub struct UndoManager {
    stacks: DashMap<String, VecDeque<UndoEntry>>,
    db: Mutex<Option<SqliteConnection>>,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Queryable)]
struct DbUndoEntry {
    id: String,
    tool_name: String,
    timestamp: String,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = undo_entries)]
struct NewUndoEntry<'a> {
    id: &'a str,
    session_id: &'a str,
    tool_name: &'a str,
    timestamp: &'a str,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = undo_operations)]
struct NewUndoOperation<'a> {
    entry_id: &'a str,
    op_type: &'a str,
    path: &'a str,
    original_content: Option<&'a [u8]>,
    sort_order: i32,
}

impl UndoManager {
    pub fn new() -> Self {
        Self {
            stacks: DashMap::new(),
            db: Mutex::new(None),
        }
    }

    /// Sets the DB path and creates tables
    pub fn set_db_path(&self, path: &Path) -> Result<(), String> {
        let path_str = path.to_str().ok_or_else(|| "Invalid DB path".to_string())?;
        let mut conn = SqliteConnection::establish(path_str)
            .map_err(|e| format!("Failed to open undo DB: {e}"))?;

        conn.batch_execute("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to enable WAL mode: {e}"))?;

        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| format!("Failed to run undo migrations: {e}"))?;

        *self.db.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn);
        Ok(())
    }

    /// Loads entries for a specific session from the DB and restores them to the in-memory stack
    fn load_session_from_db(&self, session_id: &str) -> Result<(), String> {
        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let conn = guard.as_mut().ok_or("Undo DB not initialized")?;

        let entries = undo_entries::table
            .filter(undo_entries::session_id.eq(session_id))
            .select((
                undo_entries::id,
                undo_entries::tool_name,
                undo_entries::timestamp,
            ))
            .load::<DbUndoEntry>(conn)
            .map_err(|e| format!("Failed to query undo entries: {e}"))?;

        if entries.is_empty() {
            return Ok(());
        }

        let mut stack = VecDeque::new();
        for entry_row in &entries {
            let id = Uuid::parse_str(&entry_row.id).unwrap_or_default();
            let timestamp = entry_row
                .timestamp
                .parse::<chrono::DateTime<Utc>>()
                .unwrap_or_default();

            let ops = undo_operations::table
                .filter(undo_operations::entry_id.eq(&entry_row.id))
                .order(undo_operations::sort_order.asc())
                .select((
                    undo_operations::op_type,
                    undo_operations::path,
                    undo_operations::original_content,
                    undo_operations::sort_order,
                ))
                .load::<(String, String, Option<Vec<u8>>, i32)>(conn)
                .map_err(|e| format!("Failed to query undo ops: {e}"))?
                .into_iter()
                .map(
                    |(op_type, path, original_content, _sort_order)| match op_type.as_str() {
                        "RestoreFile" => {
                            let content = original_content.map(|b| decompress(&b));
                            UndoOperation::RestoreFile {
                                path: PathBuf::from(path),
                                original_content: content,
                            }
                        }
                        _ => UndoOperation::DeleteCreatedFile {
                            path: PathBuf::from(path),
                        },
                    },
                )
                .collect();

            stack.push_back(UndoEntry {
                id,
                timestamp,
                tool_name: entry_row.tool_name.clone(),
                operations: ops,
            });
        }

        self.stacks.insert(session_id.to_string(), stack);
        Ok(())
    }

    /// Records a file restore operation
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

    /// Records a created-file deletion operation
    pub fn push_delete_created_file(&self, session_id: &str, tool_name: &str, path: PathBuf) {
        self.push(
            session_id,
            UndoEntry::new(tool_name, vec![UndoEntry::delete_created_file(path)]),
        );
    }

    /// Records an operation
    pub fn push(&self, session_id: &str, entry: UndoEntry) {
        let mut stack = self.stacks.entry(session_id.to_string()).or_default();
        stack.push_back(entry.clone());
        drop(stack);

        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.as_mut() {
            if let Err(e) = Self::persist_entry(conn, session_id, &entry) {
                tracing::error!("[UndoManager] Failed to persist undo entry: {e}");
            }
        }
    }

    fn persist_entry(
        conn: &mut SqliteConnection,
        session_id: &str,
        entry: &UndoEntry,
    ) -> Result<(), String> {
        let id_str = entry.id.to_string();
        let ts_str = entry.timestamp.to_rfc3339();

        let new_entry = NewUndoEntry {
            id: &id_str,
            session_id,
            tool_name: &entry.tool_name,
            timestamp: &ts_str,
        };

        diesel::insert_into(undo_entries::table)
            .values(&new_entry)
            .execute(conn)
            .map_err(|e| format!("Failed to insert undo entry: {e}"))?;

        for (i, op) in entry.operations.iter().enumerate() {
            match op {
                UndoOperation::RestoreFile {
                    path,
                    original_content,
                } => {
                    let compressed = original_content.as_ref().map(|c| compress(c));
                    let path_str = path.to_string_lossy();
                    let new_op = NewUndoOperation {
                        entry_id: &id_str,
                        op_type: "RestoreFile",
                        path: &path_str,
                        original_content: compressed.as_deref(),
                        sort_order: i as i32,
                    };
                    diesel::insert_into(undo_operations::table)
                        .values(&new_op)
                        .execute(conn)
                        .map_err(|e| format!("Failed to insert undo op: {e}"))?;
                }
                UndoOperation::DeleteCreatedFile { path } => {
                    let path_str = path.to_string_lossy();
                    let new_op = NewUndoOperation {
                        entry_id: &id_str,
                        op_type: "DeleteCreatedFile",
                        path: &path_str,
                        original_content: None,
                        sort_order: i as i32,
                    };
                    diesel::insert_into(undo_operations::table)
                        .values(&new_op)
                        .execute(conn)
                        .map_err(|e| format!("Failed to insert undo op: {e}"))?;
                }
            }
        }
        Ok(())
    }

    /// Undoes the most recent operation
    pub async fn undo(&self, session_id: &str) -> Result<Vec<String>, String> {
        let has_stack = self.stacks.contains_key(session_id);
        if !has_stack && self.db.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            if let Err(e) = self.load_session_from_db(session_id) {
                tracing::error!("[UndoManager] Failed to load from DB: {e}");
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

        drop(stack);

        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.as_mut() {
            if let Err(e) =
                diesel::delete(undo_operations::table.filter(undo_operations::entry_id.eq(&id_str)))
                    .execute(conn)
            {
                tracing::error!("[UndoManager] Failed to delete undo ops from DB: {e}");
            }
            if let Err(e) = diesel::delete(undo_entries::table.filter(undo_entries::id.eq(&id_str)))
                .execute(conn)
            {
                tracing::error!("[UndoManager] Failed to delete undo entry from DB: {e}");
            }
        }

        if self
            .stacks
            .get(session_id)
            .map(|s| s.is_empty())
            .unwrap_or(true)
        {
            self.stacks.remove(session_id);
        }

        Ok(logs)
    }

    /// Clears the stack
    pub fn clear(&self, session_id: &str) {
        self.stacks.remove(session_id);

        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.as_mut() {
            let target_ids: Vec<String> = undo_entries::table
                .filter(undo_entries::session_id.eq(session_id))
                .select(undo_entries::id)
                .load::<String>(conn)
                .unwrap_or_default();

            let _ = diesel::delete(
                undo_operations::table.filter(undo_operations::entry_id.eq_any(target_ids)),
            )
            .execute(conn);

            let _ =
                diesel::delete(undo_entries::table.filter(undo_entries::session_id.eq(session_id)))
                    .execute(conn);
        }
    }

    /// Gets the length of the stack
    pub fn len(&self, session_id: &str) -> usize {
        let has = self.stacks.contains_key(session_id);
        if !has && self.db.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            if let Ok(()) = self.load_session_from_db(session_id) {
                // fall through to check again
            }
        }
        self.stacks.get(session_id).map(|s| s.len()).unwrap_or(0)
    }

    /// Whether the stack is empty
    pub fn is_empty(&self, session_id: &str) -> bool {
        self.len(session_id) == 0
    }
}

/// zlib compression
fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_else(|_| data.to_vec())
}

/// zlib decompression
fn decompress(data: &[u8]) -> Vec<u8> {
    let mut decoder = ZlibDecoder::new(data);
    let mut out = Vec::new();
    let _ = decoder.read_to_end(&mut out);
    if out.is_empty() {
        return data.to_vec();
    }
    out
}

/// Takes a backup before a destructive operation
pub async fn backup_file(path: &Path) -> Option<Vec<u8>> {
    if path.exists() && path.is_file() {
        tokio::fs::read(path).await.ok()
    } else {
        None
    }
}
