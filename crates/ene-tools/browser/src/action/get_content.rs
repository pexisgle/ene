use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct GetContentArgs {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    extract: Option<String>,
    #[serde(default)]
    trim: Option<bool>,
}

/// Browser action to get DOM content.
pub struct GetContentSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl GetContentSubAction {
    /// Creates a new `GetContentSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for GetContentSubAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "get_content".to_string(),
            description: "Gets structural page content formatted as Markdown or HTML".to_string(),
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

        let args: GetContentArgs = serde_json::from_str(arguments).unwrap_or(GetContentArgs {
            format: None,
            extract: None,
            trim: None,
        });

        let format = args.format.as_deref().unwrap_or("markdown");
        let extract = args.extract.as_deref().unwrap_or("body");
        let trim = args.trim.unwrap_or(true);

        let html = page
            .content()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to get content: {e}"),
            })?;

        let extracted = match format {
            "html" => ene_tools_common::html::extract_html(&html, extract, trim),
            _ => ene_tools_common::html::extract_markdown(&html, extract, trim),
        };

        Ok(ene_tools_common::truncate::Truncate::simple(
            &extracted, 15000,
        ))
    }
}
