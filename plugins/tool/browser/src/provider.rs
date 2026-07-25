use crate::action;
use async_trait::async_trait;
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};
use ene_tool_sdk::{ActionSetProvider, ToolAction};
use std::sync::Arc;

/// Browser automation tool provider.
///
/// Dispatch is handled by [`ActionSetProvider`] (#139).
pub struct BrowserToolProvider {
    inner: ActionSetProvider,
    pub store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl BrowserToolProvider {
    /// Creates a new browser tool provider with a shared session store.
    pub fn new() -> Self {
        let store = Arc::new(crate::utils::session::BrowserSessionStore::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::NavigateAction::new(store.clone())),
            Box::new(action::ClickAction::new(store.clone())),
            Box::new(action::TypeAction::new(store.clone())),
            Box::new(action::WaitAction::new(store.clone())),
            Box::new(action::ScreenshotAction::new(store.clone())),
            Box::new(action::GetContentAction::new(store.clone())),
            Box::new(action::ScrollAction::new(store.clone())),
            Box::new(action::CloseAction::new(store.clone())),
        ];
        Self {
            inner: ActionSetProvider::new(actions),
            store,
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
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }
}
