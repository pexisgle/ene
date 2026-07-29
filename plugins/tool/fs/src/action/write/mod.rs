use crate::utils::sandbox::Sandbox;
use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_tool_sdk::prelude::*;
use std::path::Path;

pub async fn write(path: &Path, content: &str, sandbox: &Sandbox) -> Result<String, ToolError> {
    let resolved = sandbox.config().resolve_and_check(path, true)?;

    if let Some(parent) = resolved.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            ToolError::sandbox_violation(format!("Cannot create parent directory: {e}"))
        })?;
    }

    let original = if resolved.exists() {
        Some(tokio::fs::read(&resolved).await.ok()).flatten()
    } else {
        None
    };

    let content_bytes = content.as_bytes();
    let has_bom = original
        .as_ref()
        .is_some_and(|b| b.starts_with(&[0xEF, 0xBB, 0xBF]));
    let output = if has_bom {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(content_bytes);
        with_bom
    } else {
        content_bytes.to_vec()
    };

    // Check the size after BOM prepending — a 3-byte BOM added on top of
    // a content that's already at the limit used to push the write 3
    // bytes over the configured max_write_bytes.
    if output.len() > sandbox.config().max_write_bytes {
        return Err(ToolError::execution_failed(format!(
            "File too large: {} bytes exceeds maximum of {} bytes",
            output.len(),
            sandbox.config().max_write_bytes
        )));
    }

    tokio::fs::write(&resolved, output)
        .await
        .map_err(|e| ToolError::execution_failed(format!("Failed to write file: {e}")))?;

    sandbox.track_overwrite(&resolved, original).await;

    Ok("Wrote file successfully.".to_string())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "filesystem",
    name = "write",
    summary = "Write or create a file at the given path.",
    description = "Write or create a file at the given path with the specified content.",
    category = "Filesystem",
    keywords_primary = "write, create, save, file",
    side_effects = "FileSystem { mutates: true }"
)]
pub struct FsWriteAction {
    /// Absolute path to the file to write.
    file_path: String,
    /// Content to write to the file.
    content: String,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsWriteAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            file_path: String::new(),
            content: String::new(),
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            &self.file_path,
            "Writing/Overwriting file",
        )?;

        write(Path::new(&self.file_path), &self.content, &sandbox).await
    }
}
