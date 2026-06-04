use crate::utils::sandbox::SandboxConfig;
use crate::utils::undo_manager::UndoManager;
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

use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use std::sync::{Arc, RwLock};

/// Filesystem sub-action to delete a file or directory.
pub struct FsDeleteSubAction {
    sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl FsDeleteSubAction {
    /// Creates a new `FsDeleteSubAction` with the shared sandbox reference.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ToolAction for FsDeleteSubAction {
    fn tool_name(&self) -> &'static str {
        "filesystem.delete"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("filesystem.delete"),
            version: ToolVersion::default(),
            display_name: "Delete File".to_string(),
            summary: "Delete a file or directory.".to_string(),
            description: "Delete a file or directory. Directories require recursive=true."
                .to_string(),
            category: ToolCategory::Filesystem,
            keywords: KeywordSet::primary_only(["delete", "remove", "rm", "unlink"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to delete"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Required for directories (default false)"
                    }
                },
                "required": ["path"]
            }),
            examples: vec![
                ToolExample {
                    description: "Delete a single file".to_string(),
                    input: serde_json::json!({"path": "/home/user/temp.txt"}),
                    output: None,
                },
                ToolExample {
                    description: "Recursively delete a directory".to_string(),
                    input: serde_json::json!({"path": "/home/user/old_dir", "recursive": true}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
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
                message: format!("Failed to parse delete arguments: {e}"),
            })?;

        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "path is required for delete".to_string(),
            })?;
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileDelete,
            path,
            "Deleting file or directory",
        )?;

        delete(
            Path::new(path),
            recursive,
            sandbox.config(),
            undo_manager,
            &session_id,
        )
        .await
    }
}
