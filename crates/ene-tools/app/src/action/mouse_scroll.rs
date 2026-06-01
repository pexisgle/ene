use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use enigo::{Axis, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseScrollArgs {
    amount: i32,
    #[serde(default)]
    direction: Option<String>,
}

async fn run(amount: i32, direction: &str) -> Result<String, ToolError> {
    let dir_str = direction.to_string();
    let is_horizontal = direction == "left" || direction == "right";
    let scroll_amount = match direction {
        "up" => -amount,
        "down" => amount,
        "left" => -amount,
        "right" => amount,
        _ => amount,
    };
    let axis = if is_horizontal {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };

    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        enigo
            .scroll(scroll_amount, axis)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Scroll failed: {e}"),
            })?;
        Ok::<_, ToolError>(format!("Scrolled {} by {}", dir_str, amount))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to scroll the mouse.
pub struct MouseScrollAction;

#[async_trait]
impl ToolAction for MouseScrollAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "mouse_scroll".to_string(),
            description: "Simulates mouse scrolling.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "amount": { "type": "integer", "description": "Scroll steps" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] }
                },
                "required": ["amount"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["mouse".to_string(), "scroll".to_string()],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: MouseScrollArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(args.amount, args.direction.as_deref().unwrap_or("down")).await
    }
}
