use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "click",
    summary = "Clicks a page element matching the selector",
    category = "Browser",
    keywords_primary = "click, element",
    side_effects = "Browser { mutates_dom: true }"
)]
pub struct ClickAction {
    /// CSS selector for the element to click. Use only when navigate cannot reach the target.
    selector: String,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ClickAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            selector: String::new(),
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let chrome_path = crate::utils::chrome::find_chrome_executable().ok_or_else(|| ToolError::ExecutionFailed {
            message: "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.".to_string(),
        })?;

        let session = self.store.get_or_create("default", chrome_path).await?;
        let page = {
            let session_guard = session.lock().await;
            session_guard.page.clone()
        };

        page.find_element(&self.selector)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Element not found: {e}"),
            })?
            .click()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Click failed: {e}"),
            })?;

        Ok(format!("Clicked element: {}", self.selector))
    }
}
