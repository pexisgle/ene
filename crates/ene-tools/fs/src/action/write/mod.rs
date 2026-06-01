use crate::utils::sandbox::SandboxConfig;
use crate::utils::undo_manager::UndoManager;
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

use ene_tool_proto::ToolDefinition;
use ene_tools_common::ToolAction;
use std::sync::{Arc, RwLock};

/// Filesystem sub-action to write a file.
pub struct FsWriteSubAction {
    sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl FsWriteSubAction {
    /// Creates a new `FsWriteSubAction` with the shared sandbox reference.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ToolAction for FsWriteSubAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write".to_string(),
            description: "Writes a file".to_string(),
            parameters: serde_json::json!({}),
            category: None,
            keywords: vec![],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };
        let session_id = sandbox.session_id();
        let undo_manager = sandbox.undo_manager();

        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Failed to parse write arguments: {e}"),
            })?;

        let file_path = args["filePath"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "filePath is required for write".to_string(),
            })?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "content is required for write".to_string(),
            })?;

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            file_path,
            "Writing/Overwriting file",
        )?;

        write(
            Path::new(file_path),
            content,
            sandbox.config(),
            undo_manager,
            &session_id,
        )
        .await
    }
}
