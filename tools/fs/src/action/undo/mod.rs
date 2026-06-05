use crate::utils::undo_manager::UndoManager;
use ene_tool_common::prelude::*;
use std::sync::{Arc, RwLock};

pub async fn undo(undo_manager: &UndoManager, session_id: &str) -> Result<String, ToolError> {
    let logs = undo_manager
        .undo(session_id)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Undo failed: {e}"),
        })?;
    Ok(format!("Undo successful:\n{}", logs.join("\n")))
}

type SandboxRef = Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>;

fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "filesystem",
    name = "undo",
    summary = "Reverts the most recent file operation.",
    description = "Reverts the most recent file operation (write, edit, delete, patch). Can be called multiple times to undo multiple operations. Shell operations cannot be undone.",
    category = "Utility",
    keywords_primary = "undo, revert, rollback"
)]
pub struct UndoAction {
    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl UndoAction {
    pub fn new(sandbox: SandboxRef) -> Self {
        Self { sandbox }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };
        let session_id = sandbox.session_id();
        undo(sandbox.undo_manager(), &session_id).await
    }
}
