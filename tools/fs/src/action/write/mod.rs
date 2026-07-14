use crate::utils::sandbox::{Sandbox, SandboxConfig};
use ene_tool_common::prelude::*;
use std::path::Path;
use std::sync::{Arc, RwLock};

pub async fn write(path: &Path, content: &str, sandbox: &Sandbox) -> Result<String, ToolError> {
    let resolved = sandbox.config().resolve_and_check(path, true)?;

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
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "File too large: {} bytes exceeds maximum of {} bytes",
                output.len(),
                sandbox.config().max_write_bytes
            ),
        });
    }

    tokio::fs::write(&resolved, output)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to write file: {e}"),
        })?;

    sandbox.track_overwrite(&resolved, original).await;

    Ok("Wrote file successfully.".to_string())
}

type SandboxRef = Arc<RwLock<Option<Arc<Sandbox>>>>;

fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
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
        let sandbox = {
            let guard = self
                .sandbox
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .clone()
                .unwrap_or_else(|| Arc::new(Sandbox::new(SandboxConfig::default())))
        };

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            &self.file_path,
            "Writing/Overwriting file",
        )?;

        write(Path::new(&self.file_path), &self.content, &sandbox).await
    }
}
