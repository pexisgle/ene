use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use std::sync::Arc;

/// Browser action to close the session.
pub struct CloseSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl CloseSubAction {
    /// Creates a new `CloseSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for CloseSubAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "close".to_string(),
            description: "Closes the browser session".to_string(),
            parameters: serde_json::json!({}),
            category: None,
            keywords: vec![],
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        self.store.close("default");
        Ok("Browser session closed.".to_string())
    }
}
