use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct ClickArgs {
    selector: String,
}

/// Browser action to click an element.
pub struct ClickSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ClickSubAction {
    /// Creates a new `ClickSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for ClickSubAction {
    fn tool_name(&self) -> &'static str {
        "browser.click"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("browser.click"),
            version: ToolVersion::default(),
            display_name: "Clicks a page element matching the selector".to_string(),
            summary: "Clicks a page element matching the selector".to_string(),
            description: "Clicks a page element matching the selector".to_string(),
            category: ToolCategory::Browser,
            keywords: KeywordSet::primary_only(["click", "element"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the element to click. Use only when navigate cannot reach the target."
                    }
                },
                "required": ["selector"]
            }),
            examples: vec![ToolExample {
                description: "Click a button by CSS selector".to_string(),
                input: serde_json::json!({"selector": "#submit-button"}),
                output: None,
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let chrome_path = crate::utils::chrome::find_chrome_executable().ok_or_else(|| ToolError::ExecutionFailed {
            message: "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.".to_string(),
        })?;

        let session = self.store.get_or_create("default", chrome_path).await?;
        let session_guard = session.lock().await;
        let page = &session_guard.page;

        let args: ClickArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;

        page.find_element(&args.selector)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Element not found: {e}"),
            })?
            .click()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Click failed: {e}"),
            })?;

        Ok(format!("Clicked element: {}", args.selector))
    }
}
