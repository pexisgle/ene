use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct ScrollArgs {
    #[serde(default)]
    scroll_x: Option<i32>,
    #[serde(default)]
    scroll_y: Option<i32>,
}

/// Browser action to scroll the page.
pub struct ScrollSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl ScrollSubAction {
    /// Creates a new `ScrollSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for ScrollSubAction {
    fn tool_name(&self) -> &'static str {
        "browser.scroll"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("browser.scroll"),
            version: ToolVersion::default(),
            display_name: "Scrolls the page by specified pixels".to_string(),
            summary: "Scrolls the page by specified pixels".to_string(),
            description: "Scrolls the page by specified pixels".to_string(),
            category: ToolCategory::Browser,
            keywords: KeywordSet::primary_only(["scroll", "page"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "scroll_x": {
                        "type": "integer",
                        "description": "Horizontal scroll amount in pixels (default: 0)"
                    },
                    "scroll_y": {
                        "type": "integer",
                        "description": "Vertical scroll amount in pixels (default: 0)"
                    }
                }
            }),
            examples: vec![ToolExample {
                description: "Scroll down 500 pixels".to_string(),
                input: serde_json::json!({"scroll_y": 500}),
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

        let args: ScrollArgs = serde_json::from_str(arguments).unwrap_or(ScrollArgs {
            scroll_x: None,
            scroll_y: None,
        });

        let x = args.scroll_x.unwrap_or(0);
        let y = args.scroll_y.unwrap_or(0);

        let js = format!("window.scrollBy({}, {});", x, y);
        page.evaluate(js)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Scroll failed: {e}"),
            })?;

        Ok(format!("Scrolled by ({}, {})", x, y))
    }
}
