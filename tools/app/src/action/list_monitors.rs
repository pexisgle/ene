use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "list_monitors",
    summary = "Lists all connected monitors/screens with their resolutions and positions.",
    description = "Lists all connected monitors/screens with their resolutions and positions.",
    category = "App",
    keywords_primary = "monitor, screen, display, resolution"
)]
pub struct ListMonitorsAction {}

impl ListMonitorsAction {
    async fn run(&self) -> Result<String, ToolError> {
        tokio::task::spawn_blocking(move || {
            let monitors = xcap::Monitor::all().map_err(|e| {
                ToolError::execution_failed(format!("Failed to enumerate monitors: {e}"))
            })?;

            let mut result = Vec::new();
            for monitor in monitors {
                let name = monitor.name().unwrap_or_else(|_| "Unknown".to_string());
                let id = monitor.id();
                let id_val = id.map_or(serde_json::Value::Null, |v| {
                    serde_json::Value::Number(v.into())
                });
                let is_primary = monitor.is_primary().unwrap_or(false);
                let width = monitor.width().unwrap_or(0);
                let height = monitor.height().unwrap_or(0);
                let x = monitor.x().unwrap_or(0);
                let y = monitor.y().unwrap_or(0);
                let scale = monitor.scale_factor().unwrap_or(1.0);
                result.push(serde_json::json!({
                    "name": name,
                    "id": id_val,
                    "width": width,
                    "height": height,
                    "x": x,
                    "y": y,
                    "scale": scale,
                    "is_primary": is_primary,
                }));
            }
            Ok::<_, ToolError>(serde_json::json!({ "monitors": result }).to_string())
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
