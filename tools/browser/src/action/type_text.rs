use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct TypeArgs {
    selector: String,
    text: String,
}

/// Browser action to type text into an element.
pub struct TypeSubAction {
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl TypeSubAction {
    /// Creates a new `TypeSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolAction for TypeSubAction {
    fn tool_name(&self) -> &'static str {
        "browser.type"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("browser.type"),
            version: ToolVersion::default(),
            display_name: "Types text into a form element matching the selector".to_string(),
            summary: "Types text into a form element matching the selector".to_string(),
            description: "Types text into a form element matching the selector".to_string(),
            category: ToolCategory::Browser,
            keywords: KeywordSet::primary_only(["type", "input", "text"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the target input/textarea element"
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type into the element"
                    }
                },
                "required": ["selector", "text"]
            }),
            examples: vec![ToolExample {
                description: "Type into a search field".to_string(),
                input: serde_json::json!({"selector": "#search", "text": "hello world"}),
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

        let args: TypeArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;

        page.find_element(&args.selector)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Element not found: {e}"),
            })?
            .type_str(&args.text)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Type failed: {e}"),
            })?;

        Ok(format!("Typed into element: {}", args.selector))
    }
}
