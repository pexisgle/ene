use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
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
    fn tool_name(&self) -> &'static str {
        "browser.close"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("browser.close"),
            version: ToolVersion::default(),
            display_name: "Closes the browser session".to_string(),
            summary: "Closes the browser session".to_string(),
            description: "Closes the browser session".to_string(),
            category: ToolCategory::Browser,
            keywords: KeywordSet::primary_only(["close", "session"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![ToolExample {
                description: "Close the browser session".to_string(),
                input: serde_json::json!({}),
                output: None,
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        self.store.close("default");
        Ok("Browser session closed.".to_string())
    }
}
