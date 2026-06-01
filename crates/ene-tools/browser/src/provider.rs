use crate::action;
use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError, ToolProvider};
use ene_tools_common::ToolAction;
use std::collections::HashMap;

/// Returns the `ToolDefinition` for the browser tool.
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

use std::sync::Arc;

/// Dynamic ToolAction implementation for the browser tool.
pub struct BrowserAction {
    _store: Arc<crate::utils::session::BrowserSessionStore>,
    sub_actions: HashMap<String, Box<dyn ToolAction>>,
}

impl BrowserAction {
    /// Creates a new `BrowserAction` and registers all sub-actions.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::NavigateSubAction::new(store.clone())),
            Box::new(action::ClickSubAction::new(store.clone())),
            Box::new(action::TypeSubAction::new(store.clone())),
            Box::new(action::WaitSubAction::new(store.clone())),
            Box::new(action::ScreenshotSubAction::new(store.clone())),
            Box::new(action::GetContentSubAction::new(store.clone())),
            Box::new(action::ScrollSubAction::new(store.clone())),
            Box::new(action::CloseSubAction::new(store.clone())),
        ];

        let mut sub_actions = HashMap::new();
        for act in actions {
            sub_actions.insert(act.definition().name.clone(), act);
        }

        Self {
            _store: store,
            sub_actions,
        }
    }
}

#[async_trait]
impl ToolAction for BrowserAction {
    fn definition(&self) -> ToolDefinition {
        tool_definition()
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for browser: {e}"),
            })?;
        let sub_action_name =
            args["action"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "Missing 'action' field in browser arguments".to_string(),
                })?;

        if let Some(sub_act) = self.sub_actions.get(sub_action_name) {
            sub_act.execute(arguments).await
        } else {
            Err(ToolError::InvalidArguments {
                message: format!("Unknown browser action: {sub_action_name}"),
            })
        }
    }
}

/// Browser tool provider managing Chromium automation.
pub struct BrowserToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

impl BrowserToolProvider {
    /// Creates a new `BrowserToolProvider` and registers the browser action.
    pub fn new() -> Self {
        let store = Arc::new(crate::utils::session::BrowserSessionStore::new());
        Self {
            actions: vec![Box::new(BrowserAction::new(store))],
        }
    }
}

impl Default for BrowserToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for BrowserToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.actions.iter().map(|a| a.definition()).collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.definition().name == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_session_id(&self, _session_id: &str) {
        // Browser sessions are managed by BrowserSessionStore internally
    }
}
