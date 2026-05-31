use crate::sandbox::SandboxConfig;
use crate::undo_manager::UndoManager;
use ene_tool_proto::ToolError;
use std::path::Path;

pub async fn delete(
    path: &Path,
    recursive: bool,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, ToolError> {
    let resolved = sandbox.resolve_and_check(path, true)?;

    if !resolved.exists() {
        return Err(ToolError::ExecutionFailed {
            message: format!("Path not found: {}", resolved.display()),
        });
    }

    let is_dir = resolved.is_dir();

    if is_dir && !recursive {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "Path is a directory. Use recursive=true to delete directories: {}",
                resolved.display()
            ),
        });
    }

    if is_dir {
        tokio::fs::remove_dir_all(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to delete directory: {e}"),
            })?;

        undo_manager.push_delete_created_file(session_id, "delete", resolved.clone());

        Ok(format!("Deleted directory: {}", resolved.display()))
    } else {
        let original = tokio::fs::read(&resolved).await.ok();

        tokio::fs::remove_file(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to delete file: {e}"),
            })?;

        undo_manager.push_restore_file(session_id, "delete", resolved.clone(), original);

        Ok(format!("Deleted file: {}", resolved.display()))
    }
}
