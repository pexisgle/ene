use ene_tool_common::prelude::*;
use std::sync::Arc;

#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "wait",
    summary = "Waits for a specified duration in milliseconds.",
    category = "Browser",
    keywords_primary = "wait, delay, sleep"
)]
pub struct WaitAction {
    /// Milliseconds to wait (default: 1000).
    #[arg(default = "1000", minimum = 0)]
    wait_ms: Option<u64>,
}

impl WaitAction {
    pub fn new(_store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self { wait_ms: None }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let ms = self.wait_ms.unwrap_or(1000);
        tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
        Ok(format!("Waited {ms} ms"))
    }
}
