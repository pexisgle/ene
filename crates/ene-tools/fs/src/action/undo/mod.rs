use crate::utils::undo_manager::UndoManager;
use ene_tool_proto::ToolDefinition;
use ene_tool_proto::ToolError;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "undo".to_string(),
        description: "Reverts the most recent file operation (write, edit, delete, patch). Can be called multiple times to undo multiple operations. Shell operations cannot be undone.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: Some(ene_tool_proto::ToolCategory::Utility),
        keywords: vec!["undo".to_string(), "revert".to_string(), "rollback".to_string()],
    }
}

pub async fn undo(undo_manager: &UndoManager, session_id: &str) -> Result<String, ToolError> {
    let logs = undo_manager
        .undo(session_id)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Undo failed: {e}"),
        })?;
    Ok(format!("Undo successful:\n{}", logs.join("\n")))
}

/// Filesystem tool action to undo operations.
pub struct UndoAction {
    sandbox:
        std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl UndoAction {
    /// Creates a new `UndoAction` with the shared sandbox reference.
    pub fn new(
        sandbox: std::sync::Arc<
            std::sync::RwLock<Option<std::sync::Arc<crate::utils::sandbox::Sandbox>>>,
        >,
    ) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ene_tools_common::ToolAction for UndoAction {
    fn definition(&self) -> ToolDefinition {
        tool_definition()
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                std::sync::Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };
        let session_id = sandbox.session_id();
        undo(sandbox.undo_manager(), &session_id).await
    }
}
