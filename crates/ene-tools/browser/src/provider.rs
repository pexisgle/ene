use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError, ToolProvider};
use async_trait::async_trait;
use serde::Deserialize;

pub struct BrowserToolProvider {
    store: crate::session::BrowserSessionStore,
}

impl BrowserToolProvider {
    pub fn new() -> Self {
        Self {
            store: crate::session::BrowserSessionStore::new(),
        }
    }
}

#[derive(Deserialize)]
struct BrowserArgs {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    scroll_x: Option<i32>,
    #[serde(default)]
    scroll_y: Option<i32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    extract: Option<String>,
    #[serde(default)]
    trim: Option<bool>,
}

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "browser".to_string(),
        description: concat!(
            "Automates a Chromium browser via Chrome DevTools Protocol (CDP). ",
            "Supports navigation, clicking, text input, waiting for elements, taking screenshots, and extracting DOM content. ",
            "Browser state is persisted per session, so multiple actions (e.g., navigate -> wait -> get_content) can be performed sequentially. ",
            "Use the 'close' action to explicitly close the browser session.\n\n",
            "IMPORTANT: Always prefer 'navigate' over 'click' when accessing a page by URL. ",
            "Use 'click' ONLY when you must interact with page elements (buttons, links without visible URLs, form submissions) that cannot be reached via direct navigation. ",
            "Navigation is faster and more reliable than clicking.\n\n",
            "get_content output formats and data removal:\n",
            "- format='markdown' (default): Converts visible content to Markdown using html2md. ",
            "  REMOVED: All <script>, <style>, <noscript>, <iframe>, <svg>, <nav>, <header>, <footer>, <aside>, <template>, <code>, <canvas>, <audio>, <video>, <map>, <object>, <embed> elements and their contents. ",
            "  PRESERVED: Headings (h1-h6), paragraphs, lists, tables, links [text](url), images, blockquotes, and other structural content. ",
            "  Typically reduces token count to 5-15% of raw HTML.\n",
            "- format='html': Returns raw HTML. ",
            "  With trim=true (default): Same elements removed as above, returns cleaned HTML. ",
            "  With trim=false: Returns complete HTML including all scripts, styles, and hidden content. ",
            "  extract='full' with trim=false returns the entire document including <head>.\n\n",
            "extract scopes:\n",
            "- 'body' (default): Content within <body> tag only\n",
            "- 'main': Content within <main> tag (falls back to <body> if not found)\n",
            "- 'full': Entire document including <head> and <html> tags"
        ).to_string(),
        category: Some(ToolCategory::Browser),
        keywords: vec!["browser".to_string(), "web".to_string(), "navigate".to_string(), "click".to_string(), "chrome".to_string(), "scrape".to_string()],
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "type", "wait", "screenshot", "get_content", "scroll", "close"],
                    "description": "The browser action to perform"
                },
                "url": { "type": "string", "description": "URL for navigate action. Prefer navigate+URL over clicking links whenever possible." },
                "selector": { "type": "string", "description": "CSS selector for click, type, wait actions. Use only when navigate cannot reach the target." },
                "text": { "type": "string", "description": "Text to type into an element" },
                "wait_ms": { "type": "integer", "description": "Milliseconds to wait" },
                "scroll_x": { "type": "integer", "description": "Horizontal scroll amount" },
                "scroll_y": { "type": "integer", "description": "Vertical scroll amount" },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "html"],
                    "description": "Output format for get_content (default: 'markdown'). 'markdown' preserves headings/links/lists as Markdown, 'html' returns raw HTML. See full tool description for details on what is removed."
                },
                "extract": {
                    "type": "string",
                    "enum": ["body", "main", "full"],
                    "description": "Extraction scope for get_content (default: 'body'). 'body' = <body> content, 'main' = <main> content (falls back to <body>), 'full' = entire document including <head>."
                },
                "trim": {
                    "type": "boolean",
                    "description": "Remove non-content elements for get_content (default: true). When true, removes: script, style, noscript, iframe, svg, nav, header, footer, aside, template, code, canvas, audio, video, map, object, embed. Set false to keep all elements."
                }
            },
            "required": ["action"]
        }),
    }
}

#[async_trait]
impl ToolProvider for BrowserToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![tool_definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        if name != "browser" {
            return Err(ToolError::NotFound { tool_name: name.to_string() });
        }
        let args: BrowserArgs = serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
            message: format!("Invalid arguments for browser: {e}"),
        })?;
        
        let chrome_path = crate::chrome::find_chrome_executable().ok_or_else(|| {
            ToolError::ExecutionFailed {
                message: "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, ".to_string()
                    + "or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.",
            }
        })?;

        let session = self.store.get_or_create("default", chrome_path).await?;
        let session_guard = session.lock().await;
        let page = &session_guard.page;

        let result = match args.action.as_str() {
            "navigate" => {
                let url = args.url.ok_or_else(|| ToolError::InvalidArguments { message: "URL required for navigate".to_string() })?;
                page.goto(&url).await.map_err(|e| ToolError::ExecutionFailed { message: format!("Navigation failed: {e}") })?;
                let current_url = page.url().await.map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to get URL: {e}") })?.unwrap_or_else(|| url.clone());
                let title = page.evaluate("document.title").await.map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to get title: {e}") })?.into_value::<String>().unwrap_or_default();
                let ready_state = page.evaluate("document.readyState").await.map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to get readyState: {e}") })?.into_value::<String>().unwrap_or_default();
                format!("Navigation successful\nURL: {}\nTitle: {}\nReady State: {}", current_url, title, ready_state)
            }
            "click" => {
                let selector = args.selector.ok_or_else(|| ToolError::InvalidArguments { message: "Selector required for click".to_string() })?;
                page.find_element(&selector).await.map_err(|e| ToolError::ExecutionFailed { message: format!("Element not found: {e}") })?.click().await.map_err(|e| ToolError::ExecutionFailed { message: format!("Click failed: {e}") })?;
                format!("Clicked element: {}", selector)
            }
            "type" => {
                let selector = args.selector.ok_or_else(|| ToolError::InvalidArguments { message: "Selector required for type".to_string() })?;
                let text = args.text.ok_or_else(|| ToolError::InvalidArguments { message: "Text required for type".to_string() })?;
                page.find_element(&selector).await.map_err(|e| ToolError::ExecutionFailed { message: format!("Element not found: {e}") })?.type_str(&text).await.map_err(|e| ToolError::ExecutionFailed { message: format!("Type failed: {e}") })?;
                format!("Typed into element: {}", selector)
            }
            "wait" => {
                let ms = args.wait_ms.unwrap_or(1000);
                tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
                format!("Waited {} ms", ms)
            }
            "screenshot" => {
                let params = chromiumoxide::page::ScreenshotParams::default();
                let data = page.screenshot(params).await.map_err(|e| ToolError::ExecutionFailed { message: format!("Screenshot failed: {e}") })?;
                let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
                let data_uri = format!("data:image/png;base64,{}", b64);
                serde_json::json!({ "type": "screenshot", "data": data_uri }).to_string()
            }
            "get_content" => {
                let format = args.format.as_deref().unwrap_or("markdown");
                let extract = args.extract.as_deref().unwrap_or("body");
                let trim = args.trim.unwrap_or(true);
                let html = page.content().await.map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to get content: {e}") })?;
                let extracted = match format {
                    "html" => crate::extract::extract_html(&html, extract, trim),
                    _ => crate::extract::extract_markdown(&html, extract, trim),
                };
                crate::extract::truncate_text(&extracted, 15000)
            }
            "scroll" => {
                let x = args.scroll_x.unwrap_or(0);
                let y = args.scroll_y.unwrap_or(0);
                let js = format!("window.scrollBy({}, {});", x, y);
                page.evaluate(js).await.map_err(|e| ToolError::ExecutionFailed { message: format!("Scroll failed: {e}") })?;
                format!("Scrolled by ({}, {})", x, y)
            }
            "close" => {
                drop(session_guard);
                self.store.close("default");
                "Browser session closed.".to_string()
            }
            _ => return Err(ToolError::InvalidArguments { message: format!("Unknown browser action: {}", args.action) }),
        };

        Ok(result)
    }

    fn set_session_id(&self, session_id: &str) {
        // Browser sessions are managed by BrowserSessionStore internally
        // session_id could be used to namespace sessions in future
        let _ = session_id;
    }
}
