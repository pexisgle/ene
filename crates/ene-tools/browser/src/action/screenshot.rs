use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use std::sync::Arc;

/// Browser action to capture screenshot.
pub struct ScreenshotSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ScreenshotSubAction {
    /// Creates a new `ScreenshotSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for ScreenshotSubAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "screenshot".to_string(),
            description: "Takes a screenshot of the active browser tab".to_string(),
            parameters: serde_json::json!({}),
            category: None,
            keywords: vec![],
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        let chrome_path = crate::utils::chrome::find_chrome_executable().ok_or_else(|| ToolError::ExecutionFailed {
            message: "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.".to_string(),
        })?;

        let session = self.store.get_or_create("default", chrome_path).await?;
        let session_guard = session.lock().await;
        let page = &session_guard.page;

        let params = chromiumoxide::page::ScreenshotParams::default();
        let data = page
            .screenshot(params)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Screenshot failed: {e}"),
            })?;
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
        let data_uri = format!("data:image/png;base64,{}", b64);
        Ok(serde_json::json!({ "type": "screenshot", "data": data_uri }).to_string())
    }
}
