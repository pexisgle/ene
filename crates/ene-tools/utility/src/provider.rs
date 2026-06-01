use crate::action;
use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use ene_tools_common::ToolAction;
use std::sync::{Arc, Mutex};

/// Built-in utility tool provider.
///
/// Exposes four tools: `question`, `todo`, `get_current_time`, and `get_system_info`.
/// Internally uses a dynamic list of actions implementing `ToolAction`.
pub struct UtilityToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
    session_id: Arc<Mutex<String>>,
}

impl UtilityToolProvider {
    /// Creates a new `UtilityToolProvider` and registers all utility actions.
    pub fn new() -> Self {
        let todo_store = Arc::new(action::TodoStore::new());
        let session_id = Arc::new(Mutex::new("default".to_string()));

        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::AskQuestion),
            Box::new(action::UpdateTodo::new(todo_store, session_id.clone())),
            Box::new(action::GetCurrentTime),
            Box::new(action::GetSystemInfo),
        ];

        Self {
            actions,
            session_id,
        }
    }
}

impl Default for UtilityToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for UtilityToolProvider {
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

    fn set_session_id(&self, session_id: &str) {
        if let Ok(mut id) = self.session_id.lock() {
            *id = session_id.to_string();
        }
    }
}
