use ene_tool_proto::ToolError;

use super::WlCompositor;

pub(super) struct Hyprland;

impl WlCompositor for Hyprland {
    fn list_windows(&self) -> Result<String, ToolError> {
        let output = std::process::Command::new("hyprctl")
            .args(["clients", "-j"])
            .output()
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to run hyprctl: {e}"),
            })?;

        if !output.status.success() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "hyprctl failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let clients: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to parse hyprctl JSON: {e}"),
            })?;

        let mut windows = Vec::new();
        for client in &clients {
            let title = client["title"].as_str().unwrap_or("");
            let class = client["class"].as_str().unwrap_or("");
            if !title.is_empty() || !class.is_empty() {
                windows.push(format!("{title} ({class})"));
            }
        }
        Ok(windows.join("\n"))
    }

    fn focus_window(&self, title: &str) -> Result<String, ToolError> {
        let output = std::process::Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("title:{title}")])
            .output()
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to run hyprctl: {e}"),
            })?;

        if !output.status.success() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "hyprctl focus failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("Invalid") || stdout.contains("error") {
            return Err(ToolError::ExecutionFailed {
                message: format!("hyprctl focus failed for '{title}': {stdout}"),
            });
        }

        Ok(format!("Focused window matching: {title}"))
    }
}
