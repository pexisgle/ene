use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
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
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "mouse_click".to_string(),
            description: "Simulates mouse click at the current position.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button to click" },
                    "count": { "type": "integer", "description": "Click count (e.g. 1 for single, 2 for double)" }
                },
                "required": []
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["mouse".to_string(), "click".to_string()],
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
