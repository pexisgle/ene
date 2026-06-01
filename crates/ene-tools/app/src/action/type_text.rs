use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use enigo::Keyboard;
use serde::Deserialize;

#[derive(Deserialize)]
struct TypeTextArgs {
    text: String,
}

/// Action to type text.
pub struct TypeTextAction;

#[async_trait]
impl ToolAction for TypeTextAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "type_text".to_string(),
            description: "Simulates keyboard typing of a string.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to type" }
                },
                "required": ["text"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec![
                "keyboard".to_string(),
                "type".to_string(),
                "input".to_string(),
            ],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: TypeTextArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let text = args.text.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Failed to initialize enigo: {e}"),
                }
            })?;
            enigo.text(&text).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Type failed: {e}"),
            })?;
            Ok::<_, ToolError>("Text typed successfully.".to_string())
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
