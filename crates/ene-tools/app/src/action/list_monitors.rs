use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;

async fn run() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let monitors = xcap::Monitor::all().map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to enumerate monitors: {e}"),
        })?;

        let mut result = Vec::new();
        for monitor in monitors {
            let name = monitor.name().unwrap_or_else(|_| "Unknown".to_string());
            let id = monitor.id();
            let id_str = match id {
                Ok(v) => v.to_string(),
                Err(_) => "?".to_string(),
            };
            let is_primary = monitor.is_primary().unwrap_or(false);
            let width = monitor.width().unwrap_or(0);
            let height = monitor.height().unwrap_or(0);
            let x = monitor.x().unwrap_or(0);
            let y = monitor.y().unwrap_or(0);
            let scale = monitor.scale_factor().unwrap_or(1.0);
            result.push(format!(
                "{} (id: {}) {}x{} at ({},{}) scale={:.1}{}",
                name,
                id_str,
                width,
                height,
                x,
                y,
                scale,
                if is_primary { " [PRIMARY]" } else { "" }
            ));
        }
        Ok::<_, ToolError>(result.join("\n"))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to list open monitors.
pub struct ListMonitorsAction;

#[async_trait]
impl ToolAction for ListMonitorsAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_monitors".to_string(),
            description: "Lists all connected monitors/screens and their resolutions.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: Some(ToolCategory::App),
            keywords: vec![
                "monitor".to_string(),
                "screen".to_string(),
                "list".to_string(),
            ],
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        run().await
    }
}
