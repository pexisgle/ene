use ene_tool_common::prelude::*;
use std::sync::Arc;

#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "scroll",
    summary = "Scrolls the page by specified pixels",
    category = "Browser",
    keywords_primary = "scroll, page",
    side_effects = "Browser { mutates_dom: true }"
)]
pub struct ScrollAction {
    /// Horizontal scroll amount in pixels (default: 0).
    scroll_x: Option<i32>,
    /// Vertical scroll amount in pixels (default: 0).
    scroll_y: Option<i32>,

    #[tool(skip)]
    #[serde(skip, default = "crate::utils::default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ScrollAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            scroll_x: None,
            scroll_y: None,
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let page = self.store.acquire_page().await?;

        let x = self.scroll_x.unwrap_or(0);
        let y = self.scroll_y.unwrap_or(0);

        let js = format!("window.scrollBy({x}, {y});");
        page.evaluate(js)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Scroll failed: {e}")))?;

        Ok(format!("Scrolled by ({x}, {y})"))
    }
}
