use super::undo_manager::UndoManager;
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
