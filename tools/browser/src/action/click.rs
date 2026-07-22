use ene_tool_common::prelude::*;
use std::sync::Arc;

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
    #[serde(skip, default = "crate::utils::default_store")]
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
        let page = self.store.acquire_page().await?;

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
