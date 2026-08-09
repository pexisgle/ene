mod patch_parser;

use self::patch_parser::PatchOperation;
use crate::undo::UndoEntry;
use crate::utils::sandbox::Sandbox;
use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use std::path::Path;

pub async fn apply_patch(patch_text: &str, sandbox: &Sandbox) -> Result<String, ToolError> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();

    if trimmed == "*** Begin Patch\n*** End Patch" || trimmed == "*** Begin Patch\n\n*** End Patch"
    {
        return Err(ToolError::execution_failed(
            "Patch rejected: empty patch".to_string(),
        ));
    }

    if !trimmed.starts_with("*** Begin Patch") || !trimmed.ends_with("*** End Patch") {
        return Err(ToolError::execution_failed(
            "Patch must start with '*** Begin Patch' and end with '*** End Patch'".to_string(),
        ));
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
            return Err(ToolError::execution_failed(format!(
                "Unknown patch directive at line {}: {}",
                i + 1,
                line
            )));
        }
    }

    if operations.is_empty() {
        return Err(ToolError::execution_failed(
            "Patch contains no operations".to_string(),
        ));
    }

    let broker = sandbox.config().broker()?;
    let mut validated = Vec::new();
    for op in &operations {
        match op {
            PatchOperation::Update {
                path, old_content, ..
            } => {
                let resolved = sandbox.check_writable(Path::new(path))?;
                let resolved_str = resolved.to_string_lossy().into_owned();
                if broker.stat(&resolved_str).await?.is_none() {
                    return Err(ToolError::execution_failed(format!(
                        "Patch verification failed: File to update does not exist: {path}"
                    )));
                }
                let current = broker
                    .read_text(
                        &resolved_str,
                        u64::try_from(sandbox.config().max_write_bytes).unwrap_or(u64::MAX),
                    )
                    .await
                    .map_err(|e| {
                        ToolError::execution_failed(format!(
                            "Patch verification failed: Cannot read {path}: {e}"
                        ))
                    })?;
                if !current.contains(old_content) {
                    return Err(ToolError::execution_failed(format!(
                        "Patch verification failed: old_content not found in {path}. The file may have changed."
                    )));
                }
                validated.push((resolved, op.clone()));
            }
            PatchOperation::Add { path, .. } => {
                let resolved = sandbox.check_writable(Path::new(path))?;
                validated.push((resolved, op.clone()));
            }
            PatchOperation::Delete { path } => {
                let resolved = sandbox.check_writable(Path::new(path))?;
                let resolved_str = resolved.to_string_lossy().into_owned();
                if broker.stat(&resolved_str).await?.is_none() {
                    return Err(ToolError::execution_failed(format!(
                        "Patch verification failed: File to delete does not exist: {path}"
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
                let resolved_str = resolved.to_string_lossy().into_owned();
                let read_max = u64::try_from(sandbox.config().max_write_bytes).unwrap_or(u64::MAX);
                let original = broker
                    .read(&resolved_str, read_max)
                    .await
                    .ok()
                    .map(|o| o.data);
                let current = broker
                    .read_text(&resolved_str, read_max)
                    .await
                    .map_err(|e| {
                        ToolError::execution_failed(format!(
                            "Cannot read {}: {}",
                            resolved.display(),
                            e
                        ))
                    })?;

                let first = current.find(&old_content);
                let last = current.rfind(&old_content);
                if first != last {
                    return Err(ToolError::execution_failed(format!(
                        "Patch failed: old_content found multiple times in {}. Patch cannot be applied safely.",
                        resolved.display()
                    )));
                }
                let pos = first.ok_or_else(|| {
                    ToolError::execution_failed(format!(
                        "Patch failed: old_content not found in {}",
                        resolved.display()
                    ))
                })?;

                let updated =
                    current[..pos].to_string() + &new_content + &current[pos + old_content.len()..];
                broker
                    .write(&resolved_str, updated.into_bytes(), false, true)
                    .await
                    .map_err(|e| {
                        ToolError::execution_failed(format!(
                            "Cannot write {}: {}",
                            resolved.display(),
                            e
                        ))
                    })?;

                undo_ops.push(UndoEntry::restore_file(
                    resolved.display().to_string(),
                    original,
                ));
                summary.push(format!("M {}", resolved.display()));
            }
            PatchOperation::Add { content, .. } => {
                let resolved_str = resolved.to_string_lossy().into_owned();
                if let Some(parent) = resolved.parent() {
                    broker
                        .create_dir(&parent.to_string_lossy(), true)
                        .await
                        .map_err(|e| {
                            ToolError::execution_failed(format!("Cannot create directory: {e}"))
                        })?;
                }
                broker
                    .write(&resolved_str, content.as_bytes().to_vec(), true, true)
                    .await
                    .map_err(|e| {
                        ToolError::execution_failed(format!(
                            "Cannot write {}: {}",
                            resolved.display(),
                            e
                        ))
                    })?;

                undo_ops.push(UndoEntry::delete_created_file(
                    resolved.display().to_string(),
                ));
                summary.push(format!("A {}", resolved.display()));
            }
            PatchOperation::Delete { .. } => {
                let resolved_str = resolved.to_string_lossy().into_owned();
                let read_max = u64::try_from(sandbox.config().max_read_bytes).unwrap_or(u64::MAX);
                let original = if broker
                    .stat(&resolved_str)
                    .await?
                    .is_some_and(|meta| !meta.is_dir)
                {
                    broker
                        .read(&resolved_str, read_max)
                        .await
                        .ok()
                        .map(|o| o.data)
                } else {
                    None
                };

                let is_dir = broker.stat(&resolved_str).await?.is_some_and(|m| m.is_dir);
                broker.delete(&resolved_str, is_dir).await.map_err(|e| {
                    ToolError::execution_failed(format!(
                        "Cannot delete {}: {}",
                        resolved.display(),
                        e
                    ))
                })?;

                undo_ops.push(UndoEntry::restore_file(
                    resolved.display().to_string(),
                    original,
                ));
                summary.push(format!("D {}", resolved.display()));
            }
        }
    }

    sandbox.track_patch(undo_ops).await;

    Ok(format!(
        "Patch applied successfully.\n{}",
        summary.join("\n")
    ))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "filesystem",
    name = "patch",
    summary = "Apply a multi-file patch in custom format.",
    description = "Apply a multi-file patch in custom format. Supports Update, Add, and Delete file directives between *** Begin Patch / *** End Patch markers.",
    category = "Filesystem",
    keywords_primary = "patch, apply, diff, update, multi-file",
    side_effects = "FileSystem { mutates: true }"
)]
pub struct FsPatchAction {
    /// Full patch text in the custom patch format.
    patch_text: String,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsPatchAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            patch_text: String::new(),
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            "multiple files (patch)",
            "Applying patch to files",
        )?;

        apply_patch(&self.patch_text, &sandbox).await
    }
}
