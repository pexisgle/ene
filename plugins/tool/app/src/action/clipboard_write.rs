use ene_tool_sdk::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "clipboard_write",
    summary = "Writes text to the system clipboard.",
    description = "Writes text to the system clipboard.",
    category = "App",
    keywords_primary = "clipboard, write, copy",
    side_effects = "System { privileged: true }"
)]
pub struct ClipboardWriteAction {
    /// Text to write to clipboard.
    text: String,
}

impl ClipboardWriteAction {
    async fn run(&self) -> Result<String, ToolError> {
        let text = self.text.clone();
        tokio::task::spawn_blocking(move || {
            let mut clipboard = arboard::Clipboard::new().map_err(|e| {
                ToolError::execution_failed(format!("Failed to access clipboard: {e}"))
            })?;
            clipboard.set_text(&text).map_err(|e| {
                ToolError::execution_failed(format!("Failed to write clipboard: {e}"))
            })?;
            Ok::<_, ToolError>("Clipboard updated.".to_string())
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
