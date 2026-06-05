use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<crate::utils::session::BrowserSessionStore> {
    Arc::new(crate::utils::session::BrowserSessionStore::new())
}

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

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    _store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl WaitAction {
    pub fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            wait_ms: None,
            _store: store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let ms = self.wait_ms.unwrap_or(1000);
        tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
        Ok(format!("Waited {} ms", ms))
    }
}
