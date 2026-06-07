use ene_tool_common::prelude::*;
use enigo::{Axis, Mouse};

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "mouse_scroll",
    summary = "Simulates mouse scrolling in a given direction.",
    description = "Simulates mouse scrolling in a given direction.",
    category = "App",
    keywords_primary = "mouse, scroll, wheel"
)]
pub struct MouseScrollAction {
    /// Number of scroll steps.
    amount: i32,
    /// Scroll direction (default: down).
    #[arg(enum_values = "up, down, left, right")]
    #[serde(default)]
    direction: Option<String>,
}

impl MouseScrollAction {
    async fn run(&self) -> Result<String, ToolError> {
        let amount = self.amount;
        let direction = self.direction.clone().unwrap_or_else(|| "down".to_string());
        let dir_str = direction.clone();
        let is_horizontal = direction == "left" || direction == "right";
        let scroll_amount = match direction.as_str() {
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
            Ok::<_, ToolError>(format!("Scrolled {dir_str} by {amount}"))
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
