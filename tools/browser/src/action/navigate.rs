use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "navigate",
    summary = "Navigates to a URL",
    category = "Browser",
    keywords_primary = "navigate, url, goto"
)]
pub struct NavigateAction {
    /// URL to navigate to. Prefer navigate+URL over clicking links whenever possible.
    url: String,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl NavigateAction {
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            url: String::new(),
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

        page.goto(&self.url)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Navigation failed: {e}"),
            })?;

        let current_url = page
            .url()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to get URL: {e}"),
            })?
            .unwrap_or_else(|| self.url.clone());

        let title = page
            .evaluate("document.title")
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to get title: {e}"),
            })?
            .into_value::<String>()
            .unwrap_or_default();

        let ready_state = page
            .evaluate("document.readyState")
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to get readyState: {e}"),
            })?
            .into_value::<String>()
            .unwrap_or_default();

        Ok(format!(
            "Navigation successful\nURL: {current_url}\nTitle: {title}\nReady State: {ready_state}"
        ))
    }
}
