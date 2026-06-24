use ene_tool_common::prelude::*;
use enigo::Keyboard;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "press_key",
    summary = "Simulates pressing and releasing a single key.",
    description = "Simulates pressing and releasing a single key.",
    category = "App",
    keywords_primary = "keyboard, press, key",
    side_effects = "System { privileged: true }"
)]
pub struct PressKeyAction {
    /// Key name (e.g., 'return', 'escape', 'tab', 'space', 'up', 'down',
    /// 'f1'-'f12').
    key: String,
}

impl PressKeyAction {
    async fn run(&self) -> Result<String, ToolError> {
        let key = self.key.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Failed to initialize enigo: {e}"),
                }
            })?;

            if let Some(enigo_key) = crate::utils::parse_key(&key) {
                enigo.key(enigo_key, enigo::Direction::Click).map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Key press failed: {e}"),
                    }
                })?;
                Ok::<_, ToolError>(format!("Pressed key: {key}"))
            } else {
                Err(ToolError::ExecutionFailed {
                    message: format!("Unsupported key: {key}"),
                })
            }
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
