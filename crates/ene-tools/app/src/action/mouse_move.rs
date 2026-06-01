use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use enigo::{Coordinate, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseMoveArgs {
    x: i32,
    y: i32,
    #[serde(default)]
    relative: Option<bool>,
}

async fn run(x: i32, y: i32, relative: bool) -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        let coord = if relative {
            Coordinate::Rel
        } else {
            Coordinate::Abs
        };
        enigo
            .move_mouse(x, y, coord)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;
        let mode = if relative { "relative" } else { "absolute" };
        Ok::<_, ToolError>(format!("Mouse moved to ({}, {}) [{}]", x, y, mode))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to move the mouse cursor.
pub struct MouseMoveAction;

#[async_trait]
impl ToolAction for MouseMoveAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "mouse_move".to_string(),
            description: "Moves the mouse cursor to absolute or relative coordinates.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "X coordinate" },
                    "y": { "type": "integer", "description": "Y coordinate" },
                    "relative": { "type": "boolean", "description": "Move relative to current position" }
                },
                "required": ["x", "y"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["mouse".to_string(), "move".to_string()],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: MouseMoveArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(args.x, args.y, args.relative.unwrap_or(false)).await
    }
}
