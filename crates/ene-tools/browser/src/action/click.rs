use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
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
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "click".to_string(),
            description: "Clicks a page element matching the selector".to_string(),
            parameters: serde_json::json!({}),
            category: None,
            keywords: vec![],
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
