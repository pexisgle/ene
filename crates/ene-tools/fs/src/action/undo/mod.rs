use crate::utils::undo_manager::UndoManager;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};

pub fn tool_definition() -> ToolSpec {
    ToolSpec {
        name: ToolName::new("utility.undo"),
        version: ToolVersion::default(),
        display_name: "Undo".to_string(),
        summary: "Reverts the most recent file operation.".to_string(),
        description: "Reverts the most recent file operation (write, edit, delete, patch). Can be called multiple times to undo multiple operations. Shell operations cannot be undone.".to_string(),
        category: ToolCategory::Utility,
        keywords: KeywordSet::primary_only(["undo", "revert", "rollback"]),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        examples: vec![
            ToolExample {
                description: "Undo the most recent file operation".to_string(),
                input: serde_json::json!({}),
                output: Some("Undo successful:\nRestored /home/user/file.txt".to_string()),
            },
        ],
        caveats: Vec::new(),
        side_effects: SideEffects::default(),
        preconditions: Vec::new(),
        related: Vec::new(),
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
    fn tool_name(&self) -> &'static str {
        "filesystem.undo"
    }

    fn definition(&self) -> ToolSpec {
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
