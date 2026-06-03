use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
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
    fn tool_name(&self) -> &'static str {
        "browser.get_content"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("browser.get_content"),
            version: ToolVersion::default(),
            display_name: "Gets structural page content formatted as Markdown or HTML".to_string(),
            summary: "Gets structural page content formatted as Markdown or HTML".to_string(),
            description: "Gets structural page content formatted as Markdown or HTML".to_string(),
            category: ToolCategory::Browser,
            keywords: KeywordSet::primary_only(["content", "dom", "html", "markdown"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "html"],
                        "description": "Output format (default: 'markdown'). 'markdown' preserves headings/links/lists as Markdown, 'html' returns raw HTML."
                    },
                    "extract": {
                        "type": "string",
                        "enum": ["body", "main", "full"],
                        "description": "Extraction scope (default: 'body'). 'body' = <body> content, 'main' = <main> content (falls back to <body>), 'full' = entire document including <head>."
                    },
                    "trim": {
                        "type": "boolean",
                        "description": "Remove non-content elements (default: true). When true, removes: script, style, noscript, iframe, svg, nav, header, footer, aside, template, code, canvas, audio, video, map, object, embed."
                    }
                }
            }),
            examples: vec![
                ToolExample {
                    description: "Get page content as Markdown".to_string(),
                    input: serde_json::json!({"format": "markdown"}),
                    output: Some("# Page Title\n\nContent here...".to_string()),
                },
                ToolExample {
                    description: "Get raw HTML of the main content area".to_string(),
                    input: serde_json::json!({"format": "html", "extract": "main", "trim": false}),
                    output: None,
                },
            ],
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
