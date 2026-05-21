use super::definition::{ToolDefinition, ToolRegistry};
use super::utility::undo_manager::UndoManager;
use crate::sandbox::SandboxConfig;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

pub struct EneToolRegistry {
    sandbox: SandboxConfig,
    undo_manager: UndoManager,
    session_id: Arc<Mutex<String>>,
}

impl EneToolRegistry {
    pub fn new(sandbox: SandboxConfig) -> Self {
        Self {
            sandbox,
            undo_manager: UndoManager::new(),
            session_id: Arc::new(Mutex::new("default".to_string())),
        }
    }

    pub fn undo_manager(&self) -> &UndoManager {
        &self.undo_manager
    }

    fn current_session_id(&self) -> String {
        self.session_id.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolRegistry for EneToolRegistry {
    fn set_session_id(&self, session_id: &str) {
        if let Ok(mut id) = self.session_id.lock() {
            *id = session_id.to_string();
        }
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        // All tools migrated to IPC host
        Vec::new()
    }

    async fn call_tool(&self, name: &str, _arguments: &str) -> Result<String, String> {
        Err(format!(
            "Tool '{}' is no longer available as a built-in; use the IPC tool host instead",
            name
        ))
    }
}
