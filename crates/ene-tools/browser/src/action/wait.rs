use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct WaitArgs {
    #[serde(default)]
    wait_ms: Option<u64>,
}

/// Browser action to wait for a specific duration.
pub struct WaitSubAction {
    _store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl WaitSubAction {
    /// Creates a new `WaitSubAction` with the shared session store.
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { _store: store }
    }
}

#[async_trait]
impl ToolAction for WaitSubAction {
    fn tool_name(&self) -> &'static str {
        "browser.wait"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("browser.wait"),
            version: ToolVersion::default(),
            display_name: "Waits for a specified duration in milliseconds".to_string(),
            summary: "Waits for a specified duration in milliseconds".to_string(),
            description: "Waits for a specified duration in milliseconds".to_string(),
            category: ToolCategory::Browser,
            keywords: KeywordSet::primary_only(["wait", "delay", "sleep"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "wait_ms": {
                        "type": "integer",
                        "description": "Milliseconds to wait (default: 1000)"
                    }
                }
            }),
            examples: vec![ToolExample {
                description: "Wait 2 seconds for page to load".to_string(),
                input: serde_json::json!({"wait_ms": 2000}),
                output: None,
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: WaitArgs = serde_json::from_str(arguments).unwrap_or(WaitArgs { wait_ms: None });
        let ms = args.wait_ms.unwrap_or(1000);
        tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
        Ok(format!("Waited {} ms", ms))
    }
}
