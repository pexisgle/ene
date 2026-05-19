use super::definition::ToolDefinition;
use super::undo_manager::UndoManager;
use crate::error::AiCoreError;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "undo".to_string(),
        description: "Reverts the most recent file operation (write, edit, delete, patch). Can be called multiple times to undo multiple operations. Shell operations cannot be undone.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: Some(super::ToolCategory::Utility),
        keywords: vec!["undo".to_string(), "revert".to_string(), "rollback".to_string()],
    }
}

pub async fn undo(undo_manager: &UndoManager, session_id: &str) -> Result<String, AiCoreError> {
    let logs = undo_manager
        .undo(session_id)
        .await
        .map_err(AiCoreError::UndoError)?;
    Ok(format!("Undo successful:\n{}", logs.join("\n")))
}
