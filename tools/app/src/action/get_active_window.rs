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
            let active_win = active_win_pos_rs::get_active_window().map_err(|()| {
                ToolError::execution_failed("Failed to get active window".to_string())
            })?;
            let result = serde_json::json!({
                "title": active_win.title,
                "app_name": active_win.app_name,
                "x": active_win.position.x,
                "y": active_win.position.y,
                "width": active_win.position.width,
                "height": active_win.position.height,
            });
            Ok::<_, ToolError>(result.to_string())
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
