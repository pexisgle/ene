use ene_tool_common::prelude::*;
use enigo::{Button, Direction, Mouse};

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "mouse_click",
    summary = "Simulates mouse click at the current cursor position.",
    description = "Simulates mouse click at the current cursor position.",
    category = "App",
    keywords_primary = "mouse, click, button",
    side_effects = "System { privileged: true }"
)]
pub struct MouseClickAction {
    /// Mouse button to click (default: left).
    #[arg(enum_values = "left, right, middle, back, forward")]
    #[serde(default)]
    button: Option<String>,
    /// Click count (default: 1, use 2 for double-click).
    #[arg(minimum = 1)]
    #[serde(default)]
    count: Option<u32>,
}

impl MouseClickAction {
    async fn run(&self) -> Result<String, ToolError> {
        let button = self.button.clone().unwrap_or_else(|| "left".to_string());
        let count = self.count.unwrap_or(1);
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::execution_failed(format!("Failed to initialize enigo: {e}"))
            })?;
            let btn = match button.as_str() {
                "left" => Button::Left,
                "right" => Button::Right,
                "middle" => Button::Middle,
                "back" => Button::Back,
                "forward" => Button::Forward,
                other => {
                    return Err(ToolError::InvalidArguments {
                        message: format!(
                            "Invalid button '{other}'. Valid values: left, right, middle, back, forward"
                        ),
                    });
                }
            };
            for _ in 0..count {
                enigo
                    .button(btn, Direction::Click)
                    .map_err(|e| ToolError::execution_failed(format!("Mouse click failed: {e}")))?;
            }
            Ok::<_, ToolError>(format!("Mouse {button} click x{count}"))
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
