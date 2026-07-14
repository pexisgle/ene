use ene_tool_common::prelude::*;
use std::sync::{Arc, RwLock};

pub async fn undo(sandbox: &crate::utils::sandbox::Sandbox) -> Result<String, ToolError> {
    let logs = sandbox
        .undo_last()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Undo failed: {e}"),
        })?;
    Ok(format!("Undo successful:\n{logs}"))
}

use crate::utils::sandbox::SandboxConfig;

type SandboxRef = Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>;

fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
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
        let sandbox = {
            let guard = self
                .sandbox
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(SandboxConfig::default()))
            })
        };
        undo(&sandbox).await
    }
}
