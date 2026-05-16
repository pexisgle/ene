use super::definition::ToolDefinition;
use super::undo_manager::UndoManager;
use crate::error::AiCoreError;
use crate::sandbox::SandboxConfig;
use std::path::Path;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "delete".to_string(),
        description: concat!(
            "Deletes a file or directory from the local filesystem. ",
            "Use with caution. Set recursive=true to delete directories and their contents."
        )
        .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The absolute path to the file or directory to delete" },
                "recursive": { "type": "boolean", "description": "If true, delete directories recursively. Required for directories." }
            },
            "required": ["path"]
        }),
    }
}

/// ファイルまたはディレクトリを削除
pub async fn delete(
    path: &Path,
    recursive: bool,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, AiCoreError> {
    let resolved = sandbox.resolve_and_check(path, true)?;

    if !resolved.exists() {
        return Err(AiCoreError::FileNotFound(format!(
            "Path not found: {}",
            resolved.display()
        )));
    }

    let is_dir = resolved.is_dir();

    if is_dir && !recursive {
        return Err(AiCoreError::ToolExecutionError(format!(
            "Path is a directory. Use recursive=true to delete directories: {}",
            resolved.display()
        )));
    }

    if is_dir {
        // ディレクトリ削除
        tokio::fs::remove_dir_all(&resolved).await.map_err(|e| {
            AiCoreError::ToolExecutionError(format!("Failed to delete directory: {e}"))
        })?;

        undo_manager.push_delete_created_file(session_id, "delete", resolved.clone());

        Ok(format!("Deleted directory: {}", resolved.display()))
    } else {
        // ファイル削除 - バックアップを取得
        let original = tokio::fs::read(&resolved).await.ok();

        tokio::fs::remove_file(&resolved)
            .await
            .map_err(|e| AiCoreError::ToolExecutionError(format!("Failed to delete file: {e}")))?;

        undo_manager.push_restore_file(session_id, "delete", resolved.clone(), original);

        Ok(format!("Deleted file: {}", resolved.display()))
    }
}
