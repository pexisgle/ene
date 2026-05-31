use ene_tool_proto::ToolError;

pub async fn clipboard_read() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| ToolError::ExecutionFailed {
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

pub async fn clipboard_write(text: &str) -> Result<String, ToolError> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| ToolError::ExecutionFailed {
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
