use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "scroll",
    summary = "Scrolls the page by specified pixels",
    category = "Browser",
    keywords_primary = "scroll, page"
)]
pub struct ScrollAction {
    /// Horizontal scroll amount in pixels (default: 0).
    scroll_x: Option<i32>,
    /// Vertical scroll amount in pixels (default: 0).
    scroll_y: Option<i32>,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ScrollAction {
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            scroll_x: None,
            scroll_y: None,
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let chrome_path = crate::utils::chrome::find_chrome_executable().ok_or_else(|| ToolError::ExecutionFailed {
            message: "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.".to_string(),
        })?;

        let session = self.store.get_or_create("default", chrome_path).await?;
        let session_guard = session.lock().await;
        let page = &session_guard.page;

        let x = self.scroll_x.unwrap_or(0);
        let y = self.scroll_y.unwrap_or(0);

        let js = format!("window.scrollBy({x}, {y});");
        page.evaluate(js)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Scroll failed: {e}"),
            })?;

        Ok(format!("Scrolled by ({x}, {y})"))
    }
}
