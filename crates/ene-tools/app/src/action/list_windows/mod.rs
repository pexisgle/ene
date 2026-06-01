use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;

mod wayland;

async fn run() -> Result<String, ToolError> {
    if crate::utils::portal::detect_wayland() {
        return wayland::list_windows_wayland();
    }

    let windows = xcap::Window::all().map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to enumerate windows: {e}"),
    })?;

    let mut result = Vec::new();
    for window in windows {
        let title = window.title().unwrap_or_default();
        let app = window.app_name().unwrap_or_default();
        if !title.is_empty() || !app.is_empty() {
            result.push(format!("{} ({})", title, app));
        }
    }
    Ok(result.join("\n"))
}

/// Action to list open windows.
pub struct ListWindowsAction;

#[async_trait]
impl ToolAction for ListWindowsAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_windows".to_string(),
            description: "Lists all open windows with their titles and positions.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["window".to_string(), "list".to_string()],
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        run().await
    }
}
