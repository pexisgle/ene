use ene_tool_proto::ToolError;

use super::WlCompositor;

pub(super) struct Sway;

/// Escapes a string for safe embedding in sway `[title="..."]` criteria.
///
/// Sway criteria values are double-quoted; a title containing `"` or `\`
/// could break out of the criteria and inject arbitrary sway commands.
fn escape_sway_criteria(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(c),
        }
    }
    escaped
}

fn sway_find_windows(node: &serde_json::Value, windows: &mut Vec<String>) {
    let node_type = node["type"].as_str().unwrap_or("");
    if node_type == "con" || node_type == "floating_con" {
        let name = node["name"].as_str().unwrap_or("");
        let app_id = node["app_id"].as_str().unwrap_or("");
        let class = node["window_properties"]["class"].as_str().unwrap_or("");
        let has_window = node["window"].as_i64().is_some();

        if has_window || !name.is_empty() || !app_id.is_empty() || !class.is_empty() {
            let display_id = if app_id.is_empty() { class } else { app_id };
            if !name.is_empty() || !display_id.is_empty() {
                windows.push(format!("{name} ({display_id})"));
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
        let escaped = escape_sway_criteria(title);
        let criteria = format!("[title=\"{escaped}\"] focus");
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

        Ok(format!("Focused window matching: {title}"))
    }
}
