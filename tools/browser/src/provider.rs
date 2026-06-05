use crate::action;
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

pub struct BrowserToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

impl BrowserToolProvider {
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
            Box::new(action::CloseAction::new(store)),
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
            if action.name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_session_id(&self, _session_id: &str) {}
}
