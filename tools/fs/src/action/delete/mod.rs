use crate::utils::sandbox::SandboxConfig;
use crate::utils::undo_manager::UndoManager;
use ene_tool_common::prelude::*;
use std::path::Path;
use std::sync::{Arc, RwLock};

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

type SandboxRef = Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>;

fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "filesystem",
    name = "delete",
    summary = "Delete a file or directory.",
    description = "Delete a file or directory. Directories require recursive=true.",
    category = "Filesystem",
    keywords_primary = "delete, remove, rm, unlink"
)]
pub struct FsDeleteAction {
    /// Absolute path to delete.
    file_path: String,
    /// Required for directories (default false).
    #[serde(default)]
    recursive: Option<bool>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsDeleteAction {
    pub fn new(sandbox: SandboxRef) -> Self {
        Self {
            file_path: String::new(),
            recursive: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };
        let session_id = sandbox.session_id();
        let undo_manager = sandbox.undo_manager();

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileDelete,
            &self.file_path,
            "Deleting file or directory",
        )?;

        delete(
            Path::new(&self.file_path),
            self.recursive.unwrap_or(false),
            sandbox.config(),
            undo_manager,
            &session_id,
        )
        .await
    }
}
