use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;

/// Action to read text from the clipboard.
pub struct ClipboardReadAction;

#[async_trait]
impl ToolAction for ClipboardReadAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "clipboard_read".to_string(),
            description: "Reads current text from the clipboard.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: Some(ToolCategory::App),
            keywords: vec!["clipboard".to_string(), "read".to_string()],
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        tokio::task::spawn_blocking(move || {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to access clipboard: {e}"),
                })?;
            let content = clipboard
                .get_text()
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to read clipboard: {e}"),
                })?;
            Ok::<_, ToolError>(content)
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
