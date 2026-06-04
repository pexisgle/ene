use crate::action;
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

/// Browser tool provider managing Chromium automation.
pub struct BrowserToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

impl BrowserToolProvider {
    /// Creates a new `BrowserToolProvider` and registers all 8 individual browser actions.
    pub fn new() -> Self {
        let store = Arc::new(crate::utils::session::BrowserSessionStore::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::NavigateSubAction::new(store.clone())),
            Box::new(action::ClickSubAction::new(store.clone())),
            Box::new(action::TypeSubAction::new(store.clone())),
            Box::new(action::WaitSubAction::new(store.clone())),
            Box::new(action::ScreenshotSubAction::new(store.clone())),
            Box::new(action::GetContentSubAction::new(store.clone())),
            Box::new(action::ScrollSubAction::new(store.clone())),
            Box::new(action::CloseSubAction::new(store)),
        ];
        Self { actions }
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
        self.actions.iter().map(|a| a.definition()).collect()
    }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.tool_name() == name {
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
