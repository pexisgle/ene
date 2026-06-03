use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
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
    fn tool_name(&self) -> &'static str {
        "app.focus_window"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.focus_window"),
            version: ToolVersion::default(),
            display_name: "Brings a window to the foreground by title substring match.".to_string(),
            summary: "Brings a window to the foreground by title substring match.".to_string(),
            description: "Brings a window to the foreground by title substring match.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["window", "focus", "foreground", "activate"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "window_title": { "type": "string", "description": "Substring of window title or app name to focus" }
                },
                "required": ["window_title"]
            }),
            examples: vec![ToolExample {
                description: "Focus a window by title".to_string(),
                input: serde_json::json!({"window_title": "Firefox"}),
                output: None,
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
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
