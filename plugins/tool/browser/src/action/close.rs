use ene_tool_common::prelude::*;
use std::sync::Arc;

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "close",
    summary = "Closes the browser session.",
    category = "Browser",
    keywords_primary = "close, session",
    side_effects = "Browser { mutates_dom: false }"
)]
pub struct CloseAction {
    #[tool(skip)]
    #[serde(skip, default = "crate::utils::default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl CloseAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let existed = self.store.close("default").await;
        if existed {
            Ok("Browser session closed.".to_string())
        } else {
            Ok("No active browser session to close.".to_string())
        }
    }
}
