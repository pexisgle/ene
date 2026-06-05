use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "type",
    summary = "Types text into a form element matching the selector.",
    category = "Browser",
    keywords_primary = "type, input, text"
)]
pub struct TypeAction {
    /// CSS selector for the target input/textarea element.
    selector: String,
    /// Text to type into the element.
    text: String,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl TypeAction {
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            selector: String::new(),
            text: String::new(),
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

        page.find_element(&self.selector)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Element not found: {e}"),
            })?
            .type_str(&self.text)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Type failed: {e}"),
            })?;

        Ok(format!("Typed into element: {}", self.selector))
    }
}
