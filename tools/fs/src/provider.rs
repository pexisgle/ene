use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};
use std::sync::{Arc, RwLock};

use crate::utils::sandbox::Sandbox;

pub struct FsToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
    sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>,
}

impl FsToolProvider {
    pub fn new() -> Self {
        let sandbox = Arc::new(RwLock::new(None));
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(crate::action::read::FsReadAction::new(sandbox.clone())),
            Box::new(crate::action::write::FsWriteAction::new(sandbox.clone())),
            Box::new(crate::action::edit::FsEditAction::new(sandbox.clone())),
            Box::new(crate::action::delete::FsDeleteAction::new(sandbox.clone())),
            Box::new(crate::action::search::search_glob::FsGlobAction::new(
                sandbox.clone(),
            )),
            Box::new(crate::action::search::search_grep::FsGrepAction::new(
                sandbox.clone(),
            )),
            Box::new(crate::action::patch::FsPatchAction::new(sandbox.clone())),
            Box::new(crate::action::shell::ShellAction::new(sandbox.clone())),
            Box::new(crate::action::undo::UndoAction::new(sandbox.clone())),
        ];
        Self { actions, sandbox }
    }
}

impl Default for FsToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for FsToolProvider {
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

    fn set_session_id(&self, session_id: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.set_session_id(session_id);
        }
    }

    fn set_sandbox(&self, data: &ene_tool_proto::SandboxConfigData) {
        let new_sandbox = Arc::new(Sandbox::new(data.clone().into()));
        let mut guard = self.sandbox.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(new_sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.approve_request(request_id);
        }
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.allow_pattern(action, target_pattern);
        }
    }
}
