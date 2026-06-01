use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use enigo::{Button, Coordinate, Direction, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseDragArgs {
    x: i32,
    y: i32,
    x2: i32,
    y2: i32,
    #[serde(default)]
    button: Option<String>,
}

async fn run(
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    button: &str,
) -> Result<String, ToolError> {
    let button = button.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;

        enigo
            .move_mouse(start_x, start_y, Coordinate::Abs)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;

        let btn = match button.as_str() {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };
        enigo
            .button(btn, Direction::Press)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse press failed: {e}"),
            })?;

        enigo
            .move_mouse(end_x, end_y, Coordinate::Abs)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;

        enigo
            .button(btn, Direction::Release)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse release failed: {e}"),
            })?;

        Ok::<_, ToolError>(format!(
            "Dragged from ({},{}) to ({},{}) with {} button",
            start_x, start_y, end_x, end_y, button
        ))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to drag the mouse.
pub struct MouseDragAction;

#[async_trait]
impl ToolAction for MouseDragAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "mouse_drag".to_string(),
            description: "Drags from one coordinate to another.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "Start X" },
                    "y": { "type": "integer", "description": "Start Y" },
                    "x2": { "type": "integer", "description": "End X" },
                    "y2": { "type": "integer", "description": "End Y" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] }
                },
                "required": ["x", "y", "x2", "y2"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["mouse".to_string(), "drag".to_string()],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: MouseDragArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(
            args.x,
            args.y,
            args.x2,
            args.y2,
            args.button.as_deref().unwrap_or("left"),
        )
        .await
    }
}
