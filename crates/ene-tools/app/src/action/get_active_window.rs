use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
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
    fn tool_name(&self) -> &'static str {
        "app.get_active_window"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.get_active_window"),
            version: ToolVersion::default(),
            display_name: "Gets the title, app name, and position of the currently focused window."
                .to_string(),
            summary: "Gets the title, app name, and position of the currently focused window."
                .to_string(),
            description: "Gets the title, app name, and position of the currently focused window."
                .to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["window", "active", "focus", "current"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![ToolExample {
                description: "Get the active window info".to_string(),
                input: serde_json::json!({}),
                output: Some(
                    "Active window: Firefox (firefox) at (0, 0) size 1920x1080".to_string(),
                ),
            }],
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
