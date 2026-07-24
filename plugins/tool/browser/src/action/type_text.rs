use ene_tool_common::prelude::*;
use std::sync::Arc;

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "type",
    summary = "Types text into a form element matching the selector.",
    category = "Browser",
    keywords_primary = "type, input, text",
    side_effects = "Browser { mutates_dom: true }"
)]
pub struct TypeAction {
    /// CSS selector for the target input/textarea element.
    selector: String,
    /// Text to type into the element.
    text: String,

    #[tool(skip)]
    #[serde(skip, default = "crate::utils::default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl TypeAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            selector: String::new(),
            text: String::new(),
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let page = self.store.acquire_page().await?;

        page.find_element(&self.selector)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Element not found: {e}")))?
            .type_str(&self.text)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Type failed: {e}")))?;

        Ok(format!("Typed into element: {}", self.selector))
    }
}
