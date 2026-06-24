use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use ene_tool_db::{DbClient, DbFilter, DbOrderBy, DbValue, Row};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use uuid::Uuid;

use crate::schema::fs_db_schema;

#[derive(Debug, Clone)]
pub enum UndoOperation {
    RestoreFile {
        path: String,
        original_content: Option<Vec<u8>>,
    },
    DeleteCreatedFile {
        path: String,
    },
}

#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub id: String,
    pub session_id: String,
    pub tool_name: String,
    pub timestamp: String,
    pub operations: Vec<UndoOperation>,
}

impl UndoEntry {
    pub fn new(tool_name: &str, operations: Vec<UndoOperation>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            session_id: String::new(),
            tool_name: tool_name.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            operations,
        }
    }

    pub fn restore_file(
        path: impl Into<String>,
        original_content: Option<Vec<u8>>,
    ) -> UndoOperation {
        UndoOperation::RestoreFile {
            path: path.into(),
            original_content,
        }
    }

    pub fn delete_created_file(path: impl Into<String>) -> UndoOperation {
        UndoOperation::DeleteCreatedFile { path: path.into() }
    }
}

pub struct UndoManager {
    client: Arc<tokio::sync::Mutex<DbClient>>,
    session_id: String,
}

impl UndoManager {
    pub async fn new(
        socket_path: &Path,
        session_id: String,
        db_auth_token: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut client = match db_auth_token {
            Some(t) => DbClient::connect_with_token(socket_path, t).await?,
            None => DbClient::connect(socket_path).await?,
        };
        client.declare_schema(fs_db_schema()).await?;

        Ok(Self {
            client: Arc::new(tokio::sync::Mutex::new(client)),
            session_id,
        })
    }

    pub fn set_session_id(&mut self, session_id: String) {
        self.session_id = session_id;
    }

    pub async fn push(&self, mut entry: UndoEntry) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = self.client.lock().await;
        entry.session_id = self.session_id.clone();

        let mut entry_row = Row::new();
        entry_row.insert("id".to_string(), DbValue::Text(entry.id.clone()));
        entry_row.insert(
            "session_id".to_string(),
            DbValue::Text(entry.session_id.clone()),
        );
        entry_row.insert(
            "tool_name".to_string(),
            DbValue::Text(entry.tool_name.clone()),
        );
        entry_row.insert(
            "timestamp".to_string(),
            DbValue::Text(entry.timestamp.clone()),
        );

        client.insert("fs_undo_entries", entry_row).await?;

        for (sort_order, op) in entry.operations.iter().enumerate() {
            let (op_type, path, original_content) = match op {
                UndoOperation::RestoreFile {
                    path,
                    original_content,
                } => ("restore_file", path.clone(), original_content.clone()),
                UndoOperation::DeleteCreatedFile { path } => {
                    ("delete_created_file", path.clone(), None)
                }
            };

            let compressed_content = if let Some(content) = original_content {
                Some(compress(&content)?)
            } else {
                None
            };

            let mut op_row = Row::new();
            op_row.insert("entry_id".to_string(), DbValue::Text(entry.id.clone()));
            op_row.insert("op_type".to_string(), DbValue::Text(op_type.to_string()));
            op_row.insert("path".to_string(), DbValue::Text(path));
            op_row.insert(
                "original_content".to_string(),
                match compressed_content {
                    Some(c) => DbValue::Blob(c),
                    None => DbValue::Null,
                },
            );
            op_row.insert("sort_order".to_string(), DbValue::Int(sort_order as i64));

            client.insert("fs_undo_operations", op_row).await?;
        }

        Ok(())
    }

    pub async fn push_restore_file(
        &self,
        tool_name: &str,
        path: impl Into<String>,
        original_content: Option<Vec<u8>>,
    ) {
        let entry = UndoEntry::new(
            tool_name,
            vec![UndoEntry::restore_file(path, original_content)],
        );

        if let Err(e) = self.push(entry).await {
            eprintln!("[UndoManager] Failed to push restore_file: {e}");
        }
    }

    pub async fn push_delete_created_file(&self, tool_name: &str, path: impl Into<String>) {
        let entry = UndoEntry::new(tool_name, vec![UndoEntry::delete_created_file(path)]);

        if let Err(e) = self.push(entry).await {
            eprintln!("[UndoManager] Failed to push delete_created_file: {e}");
        }
    }

    /// Returns the most recent undo entry **without**
    /// removing it from the database. Callers should
    /// apply the operations and, only on success, call
    /// [`discard`](Self::discard) with the returned
    /// entry's `id` to remove it. The previous
    /// pop-then-apply ordering lost the entry on
    /// apply failure, making a retry impossible.
    ///
    /// Tiebreaking: when two entries share a timestamp
    /// the sort is nondeterministic (RFC3339 string
    /// comparison is the only distinguishing key). We
    /// add `id` as a secondary `DESC` order so the
    /// "most recent" entry is always the same one for
    /// a given `(timestamp, id)` pair.
    pub async fn peek(&self) -> Result<Option<UndoEntry>, Box<dyn std::error::Error>> {
        let mut client = self.client.lock().await;

        let filter = DbFilter::eq("session_id", self.session_id.clone());

        let entries = client
            .select(
                "fs_undo_entries",
                &["id", "tool_name", "timestamp"],
                filter,
                vec![DbOrderBy::desc("timestamp"), DbOrderBy::desc("id")],
                Some(1),
            )
            .await?;

        if entries.is_empty() {
            return Ok(None);
        }

        let entry_row = &entries[0];
        let entry_id = match entry_row.get("id") {
            Some(DbValue::Text(id)) => id.clone(),
            _ => return Ok(None),
        };

        let op_filter = DbFilter::eq("entry_id", entry_id.clone());

        let operations = client
            .select(
                "fs_undo_operations",
                &["op_type", "path", "original_content"],
                op_filter,
                vec![DbOrderBy::desc("sort_order")],
                None,
            )
            .await?;

        let mut undo_ops = Vec::new();
        for op_row in &operations {
            let op_type = match op_row.get("op_type") {
                Some(DbValue::Text(t)) => t.clone(),
                _ => continue,
            };

            let path = match op_row.get("path") {
                Some(DbValue::Text(p)) => p.clone(),
                _ => continue,
            };

            let original_content = match op_row.get("original_content") {
                Some(DbValue::Blob(b)) => Some(decompress(b)?),
                _ => None,
            };

            let op = match op_type.as_str() {
                "restore_file" => UndoOperation::RestoreFile {
                    path,
                    original_content,
                },
                "delete_created_file" => UndoOperation::DeleteCreatedFile { path },
                _ => continue,
            };

            undo_ops.push(op);
        }

        let tool_name = match entry_row.get("tool_name") {
            Some(DbValue::Text(t)) => t.clone(),
            _ => "unknown".to_string(),
        };

        let timestamp = match entry_row.get("timestamp") {
            Some(DbValue::Text(t)) => t.clone(),
            _ => Utc::now().to_rfc3339(),
        };

        Ok(Some(UndoEntry {
            id: entry_id,
            session_id: self.session_id.clone(),
            tool_name,
            timestamp,
            operations: undo_ops,
        }))
    }

    /// Removes the entry with the given `id` and its
    /// associated operations from the database. Called
    /// after a successful [`peek`](Self::peek) + apply
    /// round-trip. If the entry has already been
    /// discarded (e.g. by a duplicate call after a
    /// retry), the operation is a no-op and returns
    /// `Ok(())`.
    pub async fn discard(&self, entry_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = self.client.lock().await;

        client
            .delete(
                "fs_undo_operations",
                DbFilter::eq("entry_id", entry_id.to_string()),
            )
            .await?;

        client
            .delete("fs_undo_entries", DbFilter::eq("id", entry_id.to_string()))
            .await?;

        Ok(())
    }

    pub async fn clear(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = self.client.lock().await;

        client
            .delete(
                "fs_undo_entries",
                DbFilter::eq("session_id", self.session_id.clone()),
            )
            .await?;

        Ok(())
    }
}

fn compress(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

fn decompress(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}
