use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use enigo::{Button, Direction, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseClickArgs {
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    count: Option<u32>,
}

async fn run(button: &str, count: Option<u32>) -> Result<String, ToolError> {
    let button = button.to_string();
    let count = count.unwrap_or(1);
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        let btn = match button.as_str() {
            "right" => Button::Right,
            "middle" => Button::Middle,
            "back" => Button::Back,
            "forward" => Button::Forward,
            _ => Button::Left,
        };
        for _ in 0..count {
            enigo
                .button(btn, Direction::Click)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Mouse click failed: {e}"),
                })?;
        }
        Ok::<_, ToolError>(format!("Mouse {} click x{}", button, count))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to click the mouse.
pub struct MouseClickAction;

#[async_trait]
impl ToolAction for MouseClickAction {
    fn tool_name(&self) -> &'static str {
        "app.mouse_click"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.mouse_click"),
            version: ToolVersion::default(),
            display_name: "Simulates mouse click at the current cursor position.".to_string(),
            summary: "Simulates mouse click at the current cursor position.".to_string(),
            description: "Simulates mouse click at the current cursor position.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["mouse", "click", "button"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "button": { "type": "string", "enum": ["left", "right", "middle", "back", "forward"], "description": "Mouse button to click (default: left)" },
                    "count": { "type": "integer", "description": "Click count (default: 1, use 2 for double-click)" }
                },
                "required": []
            }),
            examples: vec![
                ToolExample {
                    description: "Left click at current position".to_string(),
                    input: serde_json::json!({"button": "left"}),
                    output: None,
                },
                ToolExample {
                    description: "Right-click context menu".to_string(),
                    input: serde_json::json!({"button": "right"}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: MouseClickArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(args.button.as_deref().unwrap_or("left"), args.count).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_enum_matches_implementation() {
        // The implementation accepts left/right/middle/back/forward
        // (see `run` body) but the LLM only sees what the schema declares.
        // The enum must cover every value the body maps explicitly.
        let def = MouseClickAction.definition();
        let allowed: Vec<&str> = def
            .parameters
            .get("properties")
            .and_then(|p| p.get("button"))
            .and_then(|b| b.get("enum"))
            .and_then(|e| e.as_array())
            .expect("button field must declare an enum array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for v in ["left", "right", "middle", "back", "forward"] {
            assert!(
                allowed.contains(&v),
                "schema enum is missing `{v}`; LLM cannot call it"
            );
        }
    }
}
