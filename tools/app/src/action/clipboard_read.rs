use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "clipboard_read",
    summary = "Reads current text from the system clipboard.",
    description = "Reads current text from the system clipboard.",
    category = "App",
    keywords_primary = "clipboard, read, paste"
)]
pub struct ClipboardReadAction {}

impl ClipboardReadAction {
    async fn run(&self) -> Result<String, ToolError> {
        tokio::task::spawn_blocking(move || {
            let mut clipboard = arboard::Clipboard::new().map_err(|e| {
                ToolError::execution_failed(format!("Failed to access clipboard: {e}"))
            })?;
            let content = clipboard.get_text().map_err(|e| {
                ToolError::execution_failed(format!("Failed to read clipboard: {e}"))
            })?;
            Ok::<_, ToolError>(content)
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
