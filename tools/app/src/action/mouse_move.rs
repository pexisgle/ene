use ene_tool_common::prelude::*;
use enigo::{Coordinate, Mouse};

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "mouse_move",
    summary = "Moves the mouse cursor to absolute or relative coordinates.",
    description = "Moves the mouse cursor to absolute or relative coordinates.",
    category = "App",
    keywords_primary = "mouse, move, cursor"
)]
pub struct MouseMoveAction {
    /// X coordinate.
    x: i32,
    /// Y coordinate.
    y: i32,
    /// If true, move relative to current position (default: false).
    #[serde(default)]
    relative: Option<bool>,
}

impl MouseMoveAction {
    async fn run(&self) -> Result<String, ToolError> {
        let x = self.x;
        let y = self.y;
        let relative = self.relative.unwrap_or(false);
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
            Ok::<_, ToolError>(format!("Mouse moved to ({x}, {y}) [{mode}]"))
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
