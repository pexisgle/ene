use async_trait::async_trait;
use ene_tool_common::{ActionSetProvider, ToolAction};
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};
use std::sync::{Arc, RwLock};

use crate::utils::sandbox::Sandbox;

/// Built-in filesystem tool provider.
///
/// Dispatch is handled by [`ActionSetProvider`]; sandbox / permission state
/// is threaded through hooks (#135 / #139).
pub struct FsToolProvider {
    inner: ActionSetProvider,
}

impl FsToolProvider {
    /// Creates a new filesystem tool provider with all FS actions registered.
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

        let session_sandbox = sandbox.clone();
        let set_sandbox = sandbox.clone();
        let approve_sandbox = sandbox.clone();
        let allow_sandbox = sandbox;

        let inner = ActionSetProvider::new(actions)
            .with_session_id_hook(move |session_id| {
                let guard = session_sandbox
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(s) = guard.as_ref() {
                    s.set_session_id(session_id);
                }
            })
            .with_sandbox_hook(move |data| {
                let new_sandbox = Arc::new(Sandbox::new(data.clone().into()));
                let mut guard = set_sandbox
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *guard = Some(new_sandbox);
            })
            .with_approve_permission_hook(move |request_id| {
                let guard = approve_sandbox
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(s) = guard.as_ref() {
                    s.approve_request(request_id);
                }
            })
            .with_allow_pattern_hook(move |action, target_pattern| {
                let guard = allow_sandbox
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(s) = guard.as_ref() {
                    s.allow_pattern(action, target_pattern);
                }
            });

        Self { inner }
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
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, data: &ene_tool_proto::SandboxConfigData) {
        self.inner.set_sandbox(data);
    }

    fn approve_permission(&self, request_id: &str) {
        self.inner.approve_permission(request_id);
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.allow_pattern(action, target_pattern);
    }
}
