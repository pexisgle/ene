mod patch_parser;

use self::patch_parser::PatchOperation;
use crate::utils::sandbox::SandboxConfig;
use crate::utils::undo_manager::{UndoEntry, UndoManager};
use ene_tool_proto::ToolError;
use std::path::Path;

pub async fn apply_patch(
    patch_text: &str,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, ToolError> {
    let normalized = patch_text.replace("\r\n", "\n").replace("\r", "\n");
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

        if line.starts_with("*** Update File:") {
            let file_path = line["*** Update File:".len()..].trim();
            let start = i + 1;
            let (old_content, new_content, consumed) =
                patch_parser::parse_update_block(&lines, start)?;
            operations.push(PatchOperation::Update {
                path: file_path.to_string(),
                old_content,
                new_content,
            });
            i = start + consumed + 1;
        } else if line.starts_with("*** Add File:") {
            let file_path = line["*** Add File:".len()..].trim();
            let start = i + 1;
            let (content, consumed) = patch_parser::parse_code_block(&lines, start)?;
            operations.push(PatchOperation::Add {
                path: file_path.to_string(),
                content,
            });
            i = start + consumed + 1;
        } else if line.starts_with("*** Delete File:") {
            let file_path = line["*** Delete File:".len()..].trim();
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
                            "Patch verification failed: File to update does not exist: {}",
                            path
                        ),
                    });
                }
                let current = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Patch verification failed: Cannot read {}: {}", path, e),
                    }
                })?;
                if !current.contains(old_content) {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "Patch verification failed: old_content not found in {}. The file may have changed.",
                            path
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
                            "Patch verification failed: File to delete does not exist: {}",
                            path
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

use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use std::sync::{Arc, RwLock};

/// Filesystem sub-action to apply a multi-file patch.
pub struct FsPatchSubAction {
    sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl FsPatchSubAction {
    /// Creates a new `FsPatchSubAction` with the shared sandbox reference.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ToolAction for FsPatchSubAction {
    fn tool_name(&self) -> &'static str {
        "filesystem.patch"
    }

    fn definition(&self) -> ToolSpec {
        let description = concat!(
            "Apply a multi-file patch in custom format. Supports Update, Add, ",
            "and Delete file directives between *** Begin Patch / *** End Patch markers."
        );
        ToolSpec {
            name: ToolName::new("filesystem.patch"),
            version: ToolVersion::default(),
            display_name: "Apply Patch".to_string(),
            summary: "Apply a multi-file patch in custom format.".to_string(),
            description: description.to_string(),
            category: ToolCategory::Filesystem,
            keywords: KeywordSet::primary_only(["patch", "apply", "diff", "update", "multi-file"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "patchText": {
                        "type": "string",
                        "description": "Full patch text in the custom patch format"
                    }
                },
                "required": ["patchText"]
            }),
            examples: vec![ToolExample {
                description: "Update a file via patch".to_string(),
                input: serde_json::json!({"patchText": "*** Begin Patch\n*** Update File: src/main.rs\n--- old\n+++ new\n@@ -1 +1 @@\n-old code\n+new code\n*** End Patch"}),
                output: Some(
                    "Patch applied successfully.\nM /home/user/project/src/main.rs".to_string(),
                ),
            }],
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
                message: format!("Failed to parse patch arguments: {e}"),
            })?;

        let patch_text = args["patchText"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "patchText is required for patch".to_string(),
            })?;

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            "multiple files (patch)",
            "Applying patch to files",
        )?;

        apply_patch(patch_text, sandbox.config(), undo_manager, &session_id).await
    }
}
