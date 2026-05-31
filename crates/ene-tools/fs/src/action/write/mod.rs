use crate::sandbox::SandboxConfig;
use crate::undo_manager::UndoManager;
use ene_tool_proto::ToolError;
use std::path::Path;

pub async fn write(
    path: &Path,
    content: &str,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, ToolError> {
    let resolved = sandbox.resolve_and_check(path, true)?;

    if let Some(parent) = resolved.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::SandboxViolation {
                message: format!("Cannot create parent directory: {e}"),
            })?;
    }

    let original = if resolved.exists() {
        Some(tokio::fs::read(&resolved).await.ok()).flatten()
    } else {
        None
    };

    let content_bytes = content.as_bytes();
    if content_bytes.len() > sandbox.max_write_bytes {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "File too large: {} bytes exceeds maximum of {} bytes",
                content_bytes.len(),
                sandbox.max_write_bytes
            ),
        });
    }

    let has_bom = original
        .as_ref()
        .map(|b| b.starts_with(&[0xEF, 0xBB, 0xBF]))
        .unwrap_or(false);
    let output = if has_bom {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(content_bytes);
        with_bom
    } else {
        content_bytes.to_vec()
    };

    tokio::fs::write(&resolved, output)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to write file: {e}"),
        })?;

    undo_manager.push_restore_file(session_id, "write", resolved.clone(), original);

    Ok("Wrote file successfully.".to_string())
}
