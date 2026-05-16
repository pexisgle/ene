use chrono::Utc;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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

/// セッション単位の Undo スタック
pub struct UndoManager {
    stacks: DashMap<String, VecDeque<UndoEntry>>,
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
        }
    }

    /// 操作を記録
    pub fn push(&self, session_id: &str, entry: UndoEntry) {
        let mut stack = self.stacks.entry(session_id.to_string()).or_default();
        stack.push_back(entry);
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

    /// 最新の操作を元に戻す
    pub async fn undo(&self, session_id: &str) -> Result<Vec<String>, String> {
        let mut stack = self
            .stacks
            .get_mut(session_id)
            .ok_or("No undo history for this session")?;
        let entry = stack.pop_back().ok_or("Undo stack is empty")?;

        let mut logs = Vec::new();
        logs.push(format!("Undoing {} ({})", entry.tool_name, entry.id));

        for op in entry.operations {
            match op {
                UndoOperation::RestoreFile {
                    path,
                    original_content,
                } => match original_content {
                    Some(content) => {
                        tokio::fs::write(&path, content).await.map_err(|e| {
                            format!("Failed to restore file {}: {}", path.display(), e)
                        })?;
                        logs.push(format!("Restored file: {}", path.display()));
                    }
                    None => {
                        if path.exists() {
                            tokio::fs::remove_file(&path).await.map_err(|e| {
                                format!("Failed to remove created file {}: {}", path.display(), e)
                            })?;
                            logs.push(format!("Removed created file: {}", path.display()));
                        }
                    }
                },
                UndoOperation::DeleteCreatedFile { path } => {
                    if path.exists() {
                        if path.is_file() {
                            tokio::fs::remove_file(&path).await.map_err(|e| {
                                format!("Failed to delete created file {}: {}", path.display(), e)
                            })?;
                        } else if path.is_dir() {
                            tokio::fs::remove_dir_all(&path).await.map_err(|e| {
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

        Ok(logs)
    }

    /// スタックをクリア
    pub fn clear(&self, session_id: &str) {
        self.stacks.remove(session_id);
    }

    /// スタックの長さを取得
    pub fn len(&self, session_id: &str) -> usize {
        self.stacks.get(session_id).map(|s| s.len()).unwrap_or(0)
    }

    /// 空かどうか
    pub fn is_empty(&self, session_id: &str) -> bool {
        self.len(session_id) == 0
    }
}

/// 破壊的操作前にバックアップを取得
pub async fn backup_file(path: &Path) -> Option<Vec<u8>> {
    if path.exists() && path.is_file() {
        tokio::fs::read(path).await.ok()
    } else {
        None
    }
}
