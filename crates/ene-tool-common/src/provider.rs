//! `ToolProvider` adapters over collections of [`ToolAction`]s.
//!
//! Every hand-written `ToolProvider` in the `tools/` binaries re-implements
//! the same dispatch loop: match `call_tool`'s `name` against each action's
//! `name()`, forward to `execute`, and return [`ToolError::NotFound`] on a
//! miss. [`ActionSetProvider`] and [`SingleActionProvider`] factor that loop
//! out so new tool binaries don't have to hand-write a `ToolProvider` impl
//! just to dispatch a fixed action list — see the ABI compatibility table in
//! `docs/tools/sdk.md` for how this fits into the wider tool ABI.

use crate::ToolAction;
use async_trait::async_trait;
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};

type SessionIdHook = Box<dyn Fn(&str) + Send + Sync>;
type SandboxHook = Box<dyn Fn(&SandboxConfigData) + Send + Sync>;
type PermissionHook = Box<dyn Fn(&str) + Send + Sync>;
type AllowPatternHook = Box<dyn Fn(&str, &str) + Send + Sync>;
type SetConfigHook = Box<dyn Fn(&serde_json::Value) + Send + Sync>;
type ConfigSchemaHook = Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>;

/// Adapts a flat `Vec<Box<dyn ToolAction>>` into a [`ToolProvider`].
///
/// `list_specs` and `call_tool` are dispatched generically over the action
/// list; lifecycle hooks (`set_session_id`, `set_sandbox`, permissions,
/// config) are no-ops unless registered via the corresponding `with_*`
/// builders.
pub struct ActionSetProvider {
    actions: Vec<Box<dyn ToolAction>>,
    on_session_id: Option<SessionIdHook>,
    on_sandbox: Option<SandboxHook>,
    on_approve_permission: Option<PermissionHook>,
    on_allow_pattern: Option<AllowPatternHook>,
    on_set_config: Option<SetConfigHook>,
    config_schema: Option<ConfigSchemaHook>,
}

impl ActionSetProvider {
    /// Creates a provider dispatching over the given actions.
    #[must_use]
    pub fn new(actions: Vec<Box<dyn ToolAction>>) -> Self {
        Self {
            actions,
            on_session_id: None,
            on_sandbox: None,
            on_approve_permission: None,
            on_allow_pattern: None,
            on_set_config: None,
            config_schema: None,
        }
    }

    /// Registers a callback invoked on `ToolProvider::set_session_id`.
    #[must_use]
    pub fn with_session_id_hook(mut self, hook: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_session_id = Some(Box::new(hook));
        self
    }

    /// Registers a callback invoked on `ToolProvider::set_sandbox`.
    #[must_use]
    pub fn with_sandbox_hook(
        mut self,
        hook: impl Fn(&SandboxConfigData) + Send + Sync + 'static,
    ) -> Self {
        self.on_sandbox = Some(Box::new(hook));
        self
    }

    /// Registers a callback invoked on `ToolProvider::approve_permission`.
    #[must_use]
    pub fn with_approve_permission_hook(
        mut self,
        hook: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.on_approve_permission = Some(Box::new(hook));
        self
    }

    /// Registers a callback invoked on `ToolProvider::allow_pattern`.
    #[must_use]
    pub fn with_allow_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.on_allow_pattern = Some(Box::new(hook));
        self
    }

    /// Registers a callback invoked on `ToolProvider::set_config`.
    #[must_use]
    pub fn with_set_config_hook(
        mut self,
        hook: impl Fn(&serde_json::Value) + Send + Sync + 'static,
    ) -> Self {
        self.on_set_config = Some(Box::new(hook));
        self
    }

    /// Registers a callback that returns the tool's config JSON Schema.
    #[must_use]
    pub fn with_config_schema_hook(
        mut self,
        hook: impl Fn() -> Option<serde_json::Value> + Send + Sync + 'static,
    ) -> Self {
        self.config_schema = Some(Box::new(hook));
        self
    }
}

#[async_trait]
impl ToolProvider for ActionSetProvider {
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
        if let Some(hook) = &self.on_session_id {
            hook(session_id);
        }
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        if let Some(hook) = &self.on_sandbox {
            hook(sandbox);
        }
    }

    fn approve_permission(&self, request_id: &str) {
        if let Some(hook) = &self.on_approve_permission {
            hook(request_id);
        }
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        if let Some(hook) = &self.on_allow_pattern {
            hook(action, target_pattern);
        }
    }

    fn set_config(&self, config: &serde_json::Value) {
        if let Some(hook) = &self.on_set_config {
            hook(config);
        }
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.config_schema.as_ref().and_then(|hook| hook())
    }
}

/// Adapts a single [`ToolAction`] into a [`ToolProvider`].
///
/// Convenience wrapper for the individual-tool pattern (one binary, one
/// action) — equivalent to `ActionSetProvider::new(vec![Box::new(action)])`
/// but avoids the `Vec` boilerplate at the call site. Supports the same
/// hooks as [`ActionSetProvider`].
pub struct SingleActionProvider {
    inner: ActionSetProvider,
}

impl SingleActionProvider {
    /// Creates a provider dispatching to a single action.
    #[must_use]
    pub fn new(action: impl ToolAction + 'static) -> Self {
        Self {
            inner: ActionSetProvider::new(vec![Box::new(action)]),
        }
    }

    /// Registers a callback invoked on `ToolProvider::set_session_id`.
    #[must_use]
    pub fn with_session_id_hook(self, hook: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            inner: self.inner.with_session_id_hook(hook),
        }
    }

    /// Registers a callback invoked on `ToolProvider::set_sandbox`.
    #[must_use]
    pub fn with_sandbox_hook(
        self,
        hook: impl Fn(&SandboxConfigData) + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: self.inner.with_sandbox_hook(hook),
        }
    }
}

#[async_trait]
impl ToolProvider for SingleActionProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_session_id(&self, session_id: &str) {
        self.inner.set_session_id(session_id);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        self.inner.approve_permission(request_id);
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.allow_pattern(action, target_pattern);
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.inner.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.inner.config_schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_tool_proto::{ToolName, ToolSpec};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct EchoAction;

    #[async_trait]
    impl ToolAction for EchoAction {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn definition(&self) -> ToolSpec {
            ToolSpec::new(
                ToolName::new("echo"),
                "Echoes the input back unchanged.",
                serde_json::json!({"type": "object"}),
            )
        }

        async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
            Ok(arguments.to_string())
        }
    }

    #[tokio::test]
    async fn action_set_provider_dispatches_by_name() {
        let provider = ActionSetProvider::new(vec![Box::new(EchoAction)]);
        let result = provider.call_tool("echo", "hi").await.unwrap();
        assert_eq!(result, "hi");
    }

    #[tokio::test]
    async fn action_set_provider_not_found_for_unknown_name() {
        let provider = ActionSetProvider::new(vec![Box::new(EchoAction)]);
        let err = provider.call_tool("missing", "hi").await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn action_set_provider_session_id_hook_runs() {
        let seen = Arc::new(AtomicBool::new(false));
        let seen2 = seen.clone();
        let provider = ActionSetProvider::new(vec![Box::new(EchoAction)])
            .with_session_id_hook(move |_sid| seen2.store(true, Ordering::SeqCst));
        provider.set_session_id("abc");
        assert!(seen.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn single_action_provider_dispatches() {
        let provider = SingleActionProvider::new(EchoAction);
        let result = provider.call_tool("echo", "hey").await.unwrap();
        assert_eq!(result, "hey");
        assert!(provider.call_tool("nope", "").await.is_err());
    }
}
