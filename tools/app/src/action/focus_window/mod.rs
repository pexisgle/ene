use ene_tool_common::prelude::*;

#[cfg(target_os = "linux")]
mod wayland;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "focus_window",
    summary = "Brings a window to the foreground by title substring match.",
    description = "Brings a window to the foreground by title substring match.",
    category = "App",
    keywords_primary = "window, focus, foreground, activate",
    side_effects = "System { privileged: true }"
)]
pub struct FocusWindowAction {
    /// Substring of window title or app name to focus.
    window_title: String,
}

impl FocusWindowAction {
    async fn run(&self) -> Result<String, ToolError> {
        let title = &self.window_title;
        if crate::utils::portal::detect_wayland() {
            #[cfg(target_os = "linux")]
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
                    "Found window: {win_title} ({app_name}). Focus requires platform-specific implementation."
                ));
            }
        }
        Err(ToolError::ExecutionFailed {
            message: format!("Window not found: {title}"),
        })
    }
}
