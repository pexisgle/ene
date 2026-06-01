use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use enigo::Keyboard;
use serde::Deserialize;

#[derive(Deserialize)]
struct PressKeyArgs {
    key: String,
}

/// Action to press a single key.
pub struct PressKeyAction;

#[async_trait]
impl ToolAction for PressKeyAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "press_key".to_string(),
            description: "Simulates pressing and releasing a single key.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Key name (e.g., 'return', 'escape', 'tab', 'up', 'down')" }
                },
                "required": ["key"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec![
                "keyboard".to_string(),
                "press".to_string(),
                "key".to_string(),
            ],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: PressKeyArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let key = args.key.clone();
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
                Ok::<_, ToolError>(format!("Pressed key: {}", key))
            } else {
                Err(ToolError::ExecutionFailed {
                    message: format!("Unsupported key: {}", key),
                })
            }
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
