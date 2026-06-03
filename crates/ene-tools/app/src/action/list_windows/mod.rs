use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
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
    fn tool_name(&self) -> &'static str {
        "app.list_windows"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.list_windows"),
            version: ToolVersion::default(),
            display_name: "Lists all open windows with their titles and application names."
                .to_string(),
            summary: "Lists all open windows with their titles and application names.".to_string(),
            description: "Lists all open windows with their titles and application names."
                .to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["window", "list", "enumerate"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![ToolExample {
                description: "List all open windows".to_string(),
                input: serde_json::json!({}),
                output: Some("Firefox (firefox)\nTerminal (gnome-terminal)".to_string()),
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
