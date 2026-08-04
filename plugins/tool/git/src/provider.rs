use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

use crate::sandbox::{RepoScope, SandboxRef};

/// Built-in read-only git tool provider.
///
/// Dispatch is handled by [`ActionSetProvider`]; the workspace sandbox scope
/// is threaded through hooks.
pub struct GitToolProvider {
    inner: ActionSetProvider,
}

impl GitToolProvider {
    /// Creates the read-only git tool provider and its shared sandbox scope.
    pub fn new() -> Self {
        let scope: SandboxRef = Arc::new(std::sync::RwLock::new(None));
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(crate::action::StatusAction::new(scope.clone())),
            Box::new(crate::action::DiffAction::new(scope.clone())),
            Box::new(crate::action::LogAction::new(scope.clone())),
            Box::new(crate::action::BranchAction::new(scope.clone())),
            Box::new(crate::action::RemoteAction::new(scope.clone())),
            Box::new(crate::action::BlameAction::new(scope.clone())),
        ];

        let set_sandbox = scope.clone();
        let allow_scope = scope.clone();
        let revoke_scope = scope;

        let inner = ActionSetProvider::new(actions)
            .with_sandbox_hook(move |data: &SandboxConfigData| {
                let new_scope = Arc::new(RepoScope::new(data.clone()));
                let mut guard = set_sandbox
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *guard = Some(new_scope);
            })
            .with_allow_pattern_hook(move |action, target_pattern| {
                let guard = allow_scope
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(scope) = guard.as_ref() {
                    scope.allow_pattern(action, target_pattern);
                }
            })
            .with_revoke_pattern_hook(move |action, target_pattern| {
                let guard = revoke_scope
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(scope) = guard.as_ref() {
                    scope.revoke_pattern(action, target_pattern);
                }
            });

        Self { inner }
    }
}

impl Default for GitToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for GitToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, data: &SandboxConfigData) {
        self.inner.set_sandbox(data);
    }

    fn approve_permission(&self, request_id: &str) {
        self.inner.approve_permission(request_id);
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.allow_pattern(action, target_pattern);
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.revoke_pattern(action, target_pattern);
    }
}
