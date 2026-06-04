use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};

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
    fn tool_name(&self) -> &'static str {
        "app.list_monitors"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.list_monitors"),
            version: ToolVersion::default(),
            display_name: "Lists all connected monitors/screens with their resolutions and positions.".to_string(),
            summary: "Lists all connected monitors/screens with their resolutions and positions.".to_string(),
            description: "Lists all connected monitors/screens with their resolutions and positions.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["monitor", "screen", "display", "resolution"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![
                ToolExample {
                    description: "List all monitors".to_string(),
                    input: serde_json::json!({}),
                    output: Some("eDP-1 (id: 0) 1920x1080 at (0,0) scale=1.0 [PRIMARY]\nHDMI-1 (id: 1) 2560x1440 at (1920,0) scale=1.0".to_string()),
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        run().await
    }
}
