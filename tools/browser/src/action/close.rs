use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "close",
    summary = "Closes the browser session.",
    category = "Browser",
    keywords_primary = "close, session",
    side_effects = "Browser { mutates_dom: true }"
)]
pub struct CloseAction {
    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl CloseAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }

    #[expect(
        clippy::unused_async,
        reason = "tool IPC handlers are async for uniform provider dispatch"
    )]
    async fn run(&self) -> Result<String, ToolError> {
        self.store.close("default");
        Ok("Browser session closed.".to_string())
    }
}
