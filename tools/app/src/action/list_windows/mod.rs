use ene_tool_common::prelude::*;

#[cfg(target_os = "linux")]
mod wayland;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "list_windows",
    summary = "Lists all open windows with their titles and application names.",
    description = "Lists all open windows with their titles and application names.",
    category = "App",
    keywords_primary = "window, list, enumerate"
)]
pub struct ListWindowsAction {}

impl ListWindowsAction {
    #[expect(
        clippy::unused_async,
        reason = "tool IPC handlers are async for uniform provider dispatch"
    )]
    async fn run(&self) -> Result<String, ToolError> {
        if crate::utils::portal::detect_wayland() {
            #[cfg(target_os = "linux")]
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
                result.push(serde_json::json!({
                    "title": title,
                    "app_name": app,
                }));
            }
        }
        Ok(serde_json::json!({ "windows": result }).to_string())
    }
}
