use ene_tool_common::prelude::*;

use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};

pub async fn undo(sandbox: &crate::utils::sandbox::Sandbox) -> Result<String, ToolError> {
    let logs = sandbox
        .undo_last()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Undo failed: {e}")))?;
    Ok(format!("Undo successful:\n{logs}"))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "undo",
    summary = "Reverts the most recent file operation.",
    description = "Reverts the most recent file operation (write, edit, delete, patch). Can be called multiple times to undo multiple operations. Shell operations cannot be undone.",
    category = "Utility",
    keywords_primary = "undo, revert, rollback",
    side_effects = "FileSystem { mutates: true }"
)]
pub struct UndoAction {
    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl UndoAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self { sandbox }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);
        undo(&sandbox).await
    }
}
