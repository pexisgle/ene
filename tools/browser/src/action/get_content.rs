use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "get_content",
    summary = "Gets structural page content formatted as Markdown or HTML.",
    category = "Browser",
    keywords_primary = "content, dom, html, markdown"
)]
pub struct GetContentAction {
    /// Output format (default: 'markdown'). 'markdown' preserves
    /// headings/links/lists as Markdown, 'html' returns raw HTML.
    #[arg(enum_values = "markdown, html")]
    format: Option<String>,
    /// Extraction scope (default: 'body'). 'body' = `<body>` content,
    /// 'main' = `<main>` content (falls back to `<body>`), 'full' = entire
    /// document including `<head>`.
    #[arg(enum_values = "body, main, full")]
    extract: Option<String>,
    /// Remove non-content elements (default: true). When true, removes:
    /// script, style, noscript, iframe, svg, nav, header, footer, aside,
    /// template, code, canvas, audio, video, map, object, embed.
    trim: Option<bool>,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl GetContentAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            format: None,
            extract: None,
            trim: None,
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let chrome_path = crate::utils::chrome::find_chrome_executable().ok_or_else(|| ToolError::ExecutionFailed {
            message: "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.".to_string(),
        })?;

        let session = self.store.get_or_create("default", chrome_path).await?;
        let page = {
            let session_guard = session.lock().await;
            session_guard.page.clone()
        };

        let format = self.format.as_deref().unwrap_or("markdown");
        let extract = self.extract.as_deref().unwrap_or("body");
        let trim = self.trim.unwrap_or(true);

        let html = page
            .content()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to get content: {e}"),
            })?;

        let extracted = match format {
            "html" => ene_tool_common::html::extract_html(&html, extract, trim),
            _ => ene_tool_common::html::extract_markdown(&html, extract, trim),
        };

        Ok(ene_tool_common::truncate::Truncate::simple(
            &extracted, 15000,
        ))
    }
}
