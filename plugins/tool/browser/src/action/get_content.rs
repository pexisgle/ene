use ene_tool_sdk::prelude::*;
use std::sync::Arc;

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
    #[serde(skip, default = "crate::utils::default_store")]
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
        let format = self.format.as_deref().unwrap_or("markdown");
        let extract = self.extract.as_deref().unwrap_or("body");
        let trim = self.trim.unwrap_or(true);

        match format {
            "markdown" | "html" => {}
            other => {
                return Err(ToolError::InvalidArguments {
                    message: format!("Invalid format '{other}'. Valid values: markdown, html"),
                });
            }
        }
        match extract {
            "body" | "main" | "full" => {}
            other => {
                return Err(ToolError::InvalidArguments {
                    message: format!("Invalid extract '{other}'. Valid values: body, main, full"),
                });
            }
        }

        let page = self.store.acquire_page().await?;

        let html = page
            .content()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to get content: {e}")))?;

        let extracted = match format {
            "html" => ene_tool_sdk::html::extract_html(&html, extract, trim),
            _ => ene_tool_sdk::html::extract_markdown(&html, extract, trim),
        };

        Ok(ene_tool_sdk::truncate::Truncate::simple(&extracted, 15000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> GetContentAction {
        GetContentAction::new(crate::utils::default_store())
    }

    #[tokio::test]
    async fn rejects_unknown_format() {
        let result = action().execute(r#"{"format":"htm"}"#).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "unknown format must be rejected, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn rejects_unknown_extract() {
        let result = action().execute(r#"{"extract":"sidebar"}"#).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "unknown extract must be rejected, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn rejects_case_variant_format() {
        let result = action().execute(r#"{"format":"Markdown"}"#).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "case-variant format must be rejected"
        );
    }
}
