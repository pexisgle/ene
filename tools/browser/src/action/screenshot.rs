use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "screenshot",
    summary = "Takes a screenshot of the active browser tab.",
    category = "Browser",
    keywords_primary = "screenshot, capture, image"
)]
pub struct ScreenshotAction {
    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ScreenshotAction {
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }

    async fn run(&self) -> Result<String, ToolError> {
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
        let data_uri = format!("data:image/png;base64,{b64}");
        Ok(serde_json::json!({ "type": "screenshot", "data": data_uri }).to_string())
    }
}
