use ene_tool_common::prelude::*;
use enigo::{Button, Coordinate, Direction, Mouse};

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "drag",
    summary = "Drags the mouse from one coordinate to another.",
    description = "Drags the mouse from one coordinate to another.",
    category = "App",
    keywords_primary = "mouse, drag, drop",
    side_effects = "System { privileged: true }"
)]
pub struct MouseDragAction {
    /// Start X coordinate.
    x: i32,
    /// Start Y coordinate.
    y: i32,
    /// End X coordinate.
    x2: i32,
    /// End Y coordinate.
    y2: i32,
    /// Mouse button (default: left).
    #[arg(enum_values = "left, right, middle")]
    #[serde(default)]
    button: Option<String>,
}

impl MouseDragAction {
    async fn run(&self) -> Result<String, ToolError> {
        let start_x = self.x;
        let start_y = self.y;
        let end_x = self.x2;
        let end_y = self.y2;
        let button = self.button.clone().unwrap_or_else(|| "left".to_string());
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::execution_failed(format!("Failed to initialize enigo: {e}"))
            })?;

            enigo
                .move_mouse(start_x, start_y, Coordinate::Abs)
                .map_err(|e| ToolError::execution_failed(format!("Mouse move failed: {e}")))?;

            let btn = match button.as_str() {
                "left" => Button::Left,
                "right" => Button::Right,
                "middle" => Button::Middle,
                other => {
                    return Err(ToolError::InvalidArguments {
                        message: format!(
                            "Invalid button '{other}'. Valid values: left, right, middle"
                        ),
                    });
                }
            };
            enigo
                .button(btn, Direction::Press)
                .map_err(|e| ToolError::execution_failed(format!("Mouse press failed: {e}")))?;

            enigo
                .move_mouse(end_x, end_y, Coordinate::Abs)
                .map_err(|e| ToolError::execution_failed(format!("Mouse move failed: {e}")))?;

            enigo
                .button(btn, Direction::Release)
                .map_err(|e| ToolError::execution_failed(format!("Mouse release failed: {e}")))?;

            Ok::<_, ToolError>(format!(
                "Dragged from ({start_x},{start_y}) to ({end_x},{end_y}) with {button} button"
            ))
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
