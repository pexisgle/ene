use super::definition::ToolDefinition;
use super::undo_manager::UndoManager;
use crate::error::AiCoreError;
use crate::sandbox::SandboxConfig;
use std::path::Path;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "write".to_string(),
        description: concat!(
            "Writes a file to the local filesystem. ",
            "This tool will overwrite the existing file if there is one at the provided path. ",
            "If this is an existing file, you MUST use the Read tool first to read the file's contents. ",
            "ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "filePath": { "type": "string", "description": "The absolute path to the file to write (must be absolute, not relative)" },
                "content": { "type": "string", "description": "The content to write to the file" }
            },
            "required": ["filePath", "content"]
        }),
    }
}

pub async fn write(
    path: &Path,
    content: &str,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, AiCoreError> {
    let resolved = sandbox.resolve_and_check(path, true)?;

    if let Some(parent) = resolved.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            AiCoreError::SandboxViolation(format!("Cannot create parent directory: {e}"))
        })?;
    }

    let original = if resolved.exists() {
        Some(tokio::fs::read(&resolved).await.ok()).flatten()
    } else {
        None
    };

    let content_bytes = content.as_bytes();
    if content_bytes.len() > sandbox.max_write_bytes {
        return Err(AiCoreError::FileTooLarge(
            content_bytes.len(),
            sandbox.max_write_bytes,
        ));
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
        .map_err(|e| AiCoreError::ToolExecutionError(format!("Failed to write file: {e}")))?;

    undo_manager.push_restore_file(session_id, "write", resolved.clone(), original);

    Ok("Wrote file successfully.".to_string())
}
