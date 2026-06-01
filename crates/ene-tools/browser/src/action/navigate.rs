use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct NavigateArgs {
    url: String,
}

/// Browser action to navigate to a URL.
pub struct NavigateSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl NavigateSubAction {
    /// Creates a new `NavigateSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for NavigateSubAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "navigate".to_string(),
            description: "Navigates to a URL".to_string(),
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

        let args: NavigateArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;

        page.goto(&args.url)
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
            .unwrap_or_else(|| args.url.clone());

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
            "Navigation successful\nURL: {}\nTitle: {}\nReady State: {}",
            current_url, title, ready_state
        ))
    }
}
