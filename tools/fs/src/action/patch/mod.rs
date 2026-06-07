mod patch_parser;

use self::patch_parser::PatchOperation;
use crate::utils::sandbox::SandboxConfig;
use crate::utils::undo_manager::{UndoEntry, UndoManager};
use ene_tool_common::prelude::*;
use std::path::Path;
use std::sync::{Arc, RwLock};

pub async fn apply_patch(
    patch_text: &str,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, ToolError> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();

    if trimmed == "*** Begin Patch\n*** End Patch" || trimmed == "*** Begin Patch\n\n*** End Patch"
    {
        return Err(ToolError::ExecutionFailed {
            message: "Patch rejected: empty patch".to_string(),
        });
    }

    if !trimmed.starts_with("*** Begin Patch") || !trimmed.ends_with("*** End Patch") {
        return Err(ToolError::ExecutionFailed {
            message: "Patch must start with '*** Begin Patch' and end with '*** End Patch'"
                .to_string(),
        });
    }

    let mut operations = Vec::new();
    let lines: Vec<&str> = trimmed.lines().collect();
    let mut i = 1;

    while i < lines.len() - 1 {
        let line = lines[i].trim();

        if let Some(rest) = line.strip_prefix("*** Update File:") {
            let file_path = rest.trim();
            let start = i + 1;
            let (old_content, new_content, consumed) =
                patch_parser::parse_update_block(&lines, start)?;
            operations.push(PatchOperation::Update {
                path: file_path.to_string(),
                old_content,
                new_content,
            });
            i = start + consumed + 1;
        } else if let Some(rest) = line.strip_prefix("*** Add File:") {
            let file_path = rest.trim();
            let start = i + 1;
            let (content, consumed) = patch_parser::parse_code_block(&lines, start)?;
            operations.push(PatchOperation::Add {
                path: file_path.to_string(),
                content,
            });
            i = start + consumed + 1;
        } else if let Some(rest) = line.strip_prefix("*** Delete File:") {
            let file_path = rest.trim();
            operations.push(PatchOperation::Delete {
                path: file_path.to_string(),
            });
            i += 1;
        } else if line.is_empty() {
            i += 1;
        } else {
            return Err(ToolError::ExecutionFailed {
                message: format!("Unknown patch directive at line {}: {}", i + 1, line),
            });
        }
    }

    if operations.is_empty() {
        return Err(ToolError::ExecutionFailed {
            message: "Patch contains no operations".to_string(),
        });
    }

    let mut validated = Vec::new();
    for op in &operations {
        match op {
            PatchOperation::Update {
                path, old_content, ..
            } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                if !resolved.exists() {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "Patch verification failed: File to update does not exist: {path}"
                        ),
                    });
                }
                let current = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Patch verification failed: Cannot read {path}: {e}"),
                    }
                })?;
                if !current.contains(old_content) {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "Patch verification failed: old_content not found in {path}. The file may have changed."
                        ),
                    });
                }
                validated.push((resolved, op.clone()));
            }
            PatchOperation::Add { path, .. } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                validated.push((resolved, op.clone()));
            }
            PatchOperation::Delete { path } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                if !resolved.exists() {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "Patch verification failed: File to delete does not exist: {path}"
                        ),
                    });
                }
                validated.push((resolved, op.clone()));
            }
        }
    }

    let mut undo_ops = Vec::new();
    let mut summary = Vec::new();

    for (resolved, op) in validated {
        match op {
            PatchOperation::Update {
                old_content,
                new_content,
                ..
            } => {
                let original = tokio::fs::read(&resolved).await.ok();
                let current = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Cannot read {}: {}", resolved.display(), e),
                    }
                })?;

                let first = current.find(&old_content);
                let last = current.rfind(&old_content);
                if first != last {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "Patch failed: old_content found multiple times in {}. Patch cannot be applied safely.",
                            resolved.display()
                        ),
                    });
                }
                let pos = first.ok_or_else(|| ToolError::ExecutionFailed {
                    message: format!(
                        "Patch failed: old_content not found in {}",
                        resolved.display()
                    ),
                })?;

                let updated =
                    current[..pos].to_string() + &new_content + &current[pos + old_content.len()..];
                tokio::fs::write(&resolved, updated).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Cannot write {}: {}", resolved.display(), e),
                    }
                })?;

                undo_ops.push(UndoEntry::restore_file(resolved.clone(), original));
                summary.push(format!("M {}", resolved.display()));
            }
            PatchOperation::Add { content, .. } => {
                if let Some(parent) = resolved.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        ToolError::ExecutionFailed {
                            message: format!("Cannot create directory: {e}"),
                        }
                    })?;
                }
                tokio::fs::write(&resolved, &content).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Cannot write {}: {}", resolved.display(), e),
                    }
                })?;

                undo_ops.push(UndoEntry::delete_created_file(resolved.clone()));
                summary.push(format!("A {}", resolved.display()));
            }
            PatchOperation::Delete { .. } => {
                let original = if resolved.is_file() {
                    tokio::fs::read(&resolved).await.ok()
                } else {
                    None
                };

                if resolved.is_dir() {
                    tokio::fs::remove_dir_all(&resolved).await.map_err(|e| {
                        ToolError::ExecutionFailed {
                            message: format!("Cannot delete {}: {}", resolved.display(), e),
                        }
                    })?;
                } else {
                    tokio::fs::remove_file(&resolved).await.map_err(|e| {
                        ToolError::ExecutionFailed {
                            message: format!("Cannot delete {}: {}", resolved.display(), e),
                        }
                    })?;
                }

                undo_ops.push(UndoEntry::restore_file(resolved.clone(), original));
                summary.push(format!("D {}", resolved.display()));
            }
        }
    }

    undo_manager.push(session_id, UndoEntry::new("patch", undo_ops));

    Ok(format!(
        "Patch applied successfully.\n{}",
        summary.join("\n")
    ))
}

type SandboxRef = Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>;

fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "filesystem",
    name = "patch",
    summary = "Apply a multi-file patch in custom format.",
    description = "Apply a multi-file patch in custom format. Supports Update, Add, and Delete file directives between *** Begin Patch / *** End Patch markers.",
    category = "Filesystem",
    keywords_primary = "patch, apply, diff, update, multi-file"
)]
pub struct FsPatchAction {
    /// Full patch text in the custom patch format.
    patch_text: String,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsPatchAction {
    pub fn new(sandbox: SandboxRef) -> Self {
        Self {
            patch_text: String::new(),
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self
                .sandbox
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };
        let session_id = sandbox.session_id();
        let undo_manager = sandbox.undo_manager();

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            "multiple files (patch)",
            "Applying patch to files",
        )?;

        apply_patch(
            &self.patch_text,
            sandbox.config(),
            undo_manager,
            &session_id,
        )
        .await
    }
}
