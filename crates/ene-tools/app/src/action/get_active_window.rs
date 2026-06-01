use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;

async fn run() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let active_win =
            active_win_pos_rs::get_active_window().map_err(|_| ToolError::ExecutionFailed {
                message: "Failed to get active window".to_string(),
            })?;
        Ok::<_, ToolError>(format!(
            "Active window: {} ({}) at ({}, {}) size {}x{}",
            active_win.title,
            active_win.app_name,
            active_win.position.x,
            active_win.position.y,
            active_win.position.width,
            active_win.position.height,
        ))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to get the currently active/focused window.
pub struct GetActiveWindowAction;

#[async_trait]
impl ToolAction for GetActiveWindowAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "get_active_window".to_string(),
            description: "Gets the title and app name of the currently focused window.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: Some(ToolCategory::App),
            keywords: vec![
                "window".to_string(),
                "active".to_string(),
                "focus".to_string(),
            ],
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        run().await
    }
}
