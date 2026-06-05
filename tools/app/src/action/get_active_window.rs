use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "get_active_window",
    summary = "Gets the title, app name, and position of the currently focused window.",
    description = "Gets the title, app name, and position of the currently focused window.",
    category = "App",
    keywords_primary = "window, active, focus, current"
)]
pub struct GetActiveWindowAction {}

impl GetActiveWindowAction {
    async fn run(&self) -> Result<String, ToolError> {
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
}
