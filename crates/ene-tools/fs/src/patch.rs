mod parser;

use super::definition::ToolDefinition;
use super::utility::undo_manager::{UndoEntry, UndoManager};
use crate::error::ToolError;
use crate::sandbox::SandboxConfig;
use parser::PatchOperation;
use std::path::Path;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "patch".to_string(),
        description: concat!(
            "Applies a patch to multiple files at once. ",
            "The patch text describes all changes to be made. ",
            "Use this for complex multi-file changes or when you need to add/update/delete multiple files in one operation."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "patchText": { "type": "string", "description": "The full patch text that describes all changes to be made. Format: *** Begin Patch\n*** Update File: path\n```\nold content\n```\n```\nnew content\n```\n*** Add File: path\n```\ncontent\n```\n*** Delete File: path\n*** End Patch" }
            },
            "required": ["patchText"]
        }),
        category: Some(super::ToolCategory::Filesystem),
        keywords: vec!["patch".to_string(), "multi-file".to_string(), "batch".to_string(), "change".to_string()],
    }
}

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
        return Err(ToolError::ToolExecutionError(
            "Patch rejected: empty patch".to_string(),
        ));
    }

    if !trimmed.starts_with("*** Begin Patch") || !trimmed.ends_with("*** End Patch") {
        return Err(ToolError::ToolExecutionError(
            "Patch must start with '*** Begin Patch' and end with '*** End Patch'".to_string(),
        ));
    }

    let mut operations = Vec::new();
    let lines: Vec<&str> = trimmed.lines().collect();
    let mut i = 1;

    while i < lines.len() - 1 {
        let line = lines[i].trim();

        if line.starts_with("*** Update File:") {
            let file_path = line["*** Update File:".len()..].trim();
            let start = i + 1;
            let (old_content, new_content, consumed) = parser::parse_update_block(&lines, start)?;
            operations.push(PatchOperation::Update {
                path: file_path.to_string(),
                old_content,
                new_content,
            });
            i = start + consumed + 1;
        } else if line.starts_with("*** Add File:") {
            let file_path = line["*** Add File:".len()..].trim();
            let start = i + 1;
            let (content, consumed) = parser::parse_code_block(&lines, start)?;
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
            return Err(ToolError::ToolExecutionError(format!(
                "Unknown patch directive at line {}: {}",
                i + 1,
                line
            )));
        }
    }

    if operations.is_empty() {
        return Err(ToolError::ToolExecutionError(
            "Patch contains no operations".to_string(),
        ));
    }

    let mut validated = Vec::new();
    for op in &operations {
        match op {
            PatchOperation::Update {
                path, old_content, ..
            } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                if !resolved.exists() {
                    return Err(ToolError::FileNotFound(format!(
                        "Patch verification failed: File to update does not exist: {}",
                        path
                    )));
                }
                let current = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    ToolError::ToolExecutionError(format!(
                        "Patch verification failed: Cannot read {}: {}",
                        path, e
                    ))
                })?;
                if !current.contains(old_content) {
                    return Err(ToolError::ToolExecutionError(format!(
                        "Patch verification failed: old_content not found in {}. The file may have changed.",
                        path
                    )));
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
                    return Err(ToolError::FileNotFound(format!(
                        "Patch verification failed: File to delete does not exist: {}",
                        path
                    )));
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
                    ToolError::ToolExecutionError(format!(
                        "Cannot read {}: {}",
                        resolved.display(),
                        e
                    ))
                })?;

                let first = current.find(&old_content);
                let last = current.rfind(&old_content);
                if first != last {
                    return Err(ToolError::ToolExecutionError(format!(
                        "Patch failed: old_content found multiple times in {}. Patch cannot be applied safely.",
                        resolved.display()
                    )));
                }
                let pos = first.ok_or_else(|| {
                    ToolError::ToolExecutionError(format!(
                        "Patch failed: old_content not found in {}",
                        resolved.display()
                    ))
                })?;

                let updated =
                    current[..pos].to_string() + &new_content + &current[pos + old_content.len()..];
                tokio::fs::write(&resolved, updated).await.map_err(|e| {
                    ToolError::ToolExecutionError(format!(
                        "Cannot write {}: {}",
                        resolved.display(),
                        e
                    ))
                })?;

                undo_ops.push(UndoEntry::restore_file(resolved.clone(), original));
                summary.push(format!("M {}", resolved.display()));
            }
            PatchOperation::Add { content, .. } => {
                if let Some(parent) = resolved.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        ToolError::ToolExecutionError(format!("Cannot create directory: {e}"))
                    })?;
                }
                tokio::fs::write(&resolved, &content).await.map_err(|e| {
                    ToolError::ToolExecutionError(format!(
                        "Cannot write {}: {}",
                        resolved.display(),
                        e
                    ))
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
                        ToolError::ToolExecutionError(format!(
                            "Cannot delete {}: {}",
                            resolved.display(),
                            e
                        ))
                    })?;
                } else {
                    tokio::fs::remove_file(&resolved).await.map_err(|e| {
                        ToolError::ToolExecutionError(format!(
                            "Cannot delete {}: {}",
                            resolved.display(),
                            e
                        ))
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
