use ene_plugin::prelude::*;
use std::sync::Arc;

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
    #[serde(skip, default = "crate::utils::default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ScreenshotAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let page = self.store.acquire_page().await?;

        let params = chromiumoxide::page::ScreenshotParams::default();
        let data = page
            .screenshot(params)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Screenshot failed: {e}")))?;
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
        let data_uri = format!("data:image/png;base64,{b64}");
        Ok(serde_json::json!({ "type": "screenshot", "data": data_uri }).to_string())
    }
}
