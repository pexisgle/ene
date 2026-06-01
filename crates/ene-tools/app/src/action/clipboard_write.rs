use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::Deserialize;

#[derive(Deserialize)]
struct ClipboardWriteArgs {
    text: String,
}

/// Action to write text to the clipboard.
pub struct ClipboardWriteAction;

#[async_trait]
impl ToolAction for ClipboardWriteAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "clipboard_write".to_string(),
            description: "Writes text to the clipboard.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to write" }
                },
                "required": ["text"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["clipboard".to_string(), "write".to_string()],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: ClipboardWriteArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let text = args.text.clone();
        tokio::task::spawn_blocking(move || {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to access clipboard: {e}"),
                })?;
            clipboard
                .set_text(&text)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to write clipboard: {e}"),
                })?;
            Ok::<_, ToolError>("Clipboard updated.".to_string())
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
