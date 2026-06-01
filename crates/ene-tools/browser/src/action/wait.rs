use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
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
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "wait".to_string(),
            description: "Waits for a specified duration in milliseconds".to_string(),
            parameters: serde_json::json!({}),
            category: None,
            keywords: vec![],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: WaitArgs = serde_json::from_str(arguments).unwrap_or(WaitArgs { wait_ms: None });
        let ms = args.wait_ms.unwrap_or(1000);
        tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
        Ok(format!("Waited {} ms", ms))
    }
}
