use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::Deserialize;

mod wayland;

#[derive(Deserialize)]
struct FocusWindowArgs {
    window_title: String,
}

async fn run(title: &str) -> Result<String, ToolError> {
    if crate::utils::portal::detect_wayland() {
        return wayland::focus_window_wayland(title);
    }

    let windows = xcap::Window::all().map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to enumerate windows: {e}"),
    })?;

    for window in windows {
        let win_title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        if win_title.contains(title) || app_name.contains(title) {
            return Ok(format!(
                "Found window: {} ({}). Focus requires platform-specific implementation.",
                win_title, app_name
            ));
        }
    }
    Err(ToolError::ExecutionFailed {
        message: format!("Window not found: {}", title),
    })
}

/// Action to focus a window.
pub struct FocusWindowAction;

#[async_trait]
impl ToolAction for FocusWindowAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "focus_window".to_string(),
            description: "Brings a window to the foreground by title substring match.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "window_title": { "type": "string", "description": "Substring of window title to focus" }
                },
                "required": ["window_title"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec![
                "window".to_string(),
                "focus".to_string(),
                "foreground".to_string(),
            ],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: FocusWindowArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(&args.window_title).await
    }
}
