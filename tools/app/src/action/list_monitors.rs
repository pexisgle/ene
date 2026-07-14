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
            let monitors = xcap::Monitor::all().map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to enumerate monitors: {e}"),
            })?;

            let mut result = Vec::new();
            for monitor in monitors {
                let name = monitor.name().unwrap_or_else(|_| "Unknown".to_string());
                let id = monitor.id();
                let id_str = id.map_or_else(|_| "?".to_string(), |v| v.to_string());
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
}
