use ene_tool_proto::ToolError;

use super::WlCompositor;

pub(super) struct Sway;

fn sway_find_windows(node: &serde_json::Value, windows: &mut Vec<String>) {
    let node_type = node["type"].as_str().unwrap_or("");
    if node_type == "con" || node_type == "floating_con" {
        let name = node["name"].as_str().unwrap_or("");
        let app_id = node["app_id"].as_str().unwrap_or("");
        let class = node["window_properties"]["class"].as_str().unwrap_or("");
        let has_window = node["window"].as_i64().is_some();

        if has_window || !name.is_empty() || !app_id.is_empty() || !class.is_empty() {
            let display_id = if !app_id.is_empty() { app_id } else { class };
            if !name.is_empty() || !display_id.is_empty() {
                windows.push(format!("{} ({})", name, display_id));
            }
        }
    }

    if let Some(nodes) = node["nodes"].as_array() {
        for child in nodes {
            sway_find_windows(child, windows);
        }
    }
    if let Some(floating) = node["floating_nodes"].as_array() {
        for child in floating {
            sway_find_windows(child, windows);
        }
    }
}

impl WlCompositor for Sway {
    fn list_windows(&self) -> Result<String, ToolError> {
        let output = std::process::Command::new("swaymsg")
            .args(["-t", "get_tree"])
            .output()
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to run swaymsg: {e}"),
            })?;

        if !output.status.success() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "swaymsg failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let tree: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to parse sway tree JSON: {e}"),
            })?;

        let mut windows = Vec::new();
        sway_find_windows(&tree, &mut windows);
        Ok(windows.join("\n"))
    }

    fn focus_window(&self, title: &str) -> Result<String, ToolError> {
        let criteria = format!("[title=\"{}\"] focus", title);
        let output = std::process::Command::new("swaymsg")
            .arg(&criteria)
            .output()
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to run swaymsg: {e}"),
            })?;

        if !output.status.success() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "swaymsg focus failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        Ok(format!("Focused window matching: {}", title))
    }
}
