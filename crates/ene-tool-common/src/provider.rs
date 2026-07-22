//! `ToolProvider` adapters over collections of [`ToolAction`]s.
//!
//! Every hand-written `ToolProvider` in the `tools/` binaries re-implements
//! the same dispatch loop: match `call_tool`'s `name` against each action's
//! `name()`, forward to `execute`, and return [`ToolError::NotFound`] on a
//! miss. [`ActionSetProvider`] factors that loop
//! out so new tool binaries don't have to hand-write a `ToolProvider` impl
//! just to dispatch a fixed action list — see the ABI compatibility table in
//! `docs/reference/tools/sdk.md` for how this fits into the wider tool ABI.
//!
//! ## Deferred (background) execution
//!
//! Neither [`ActionSetProvider`] nor [`SingleActionProvider`] overrides
//! `call_tool_deferred`, `poll_deferred`, or `cancel_deferred`. This is
//! **intentional**: the `ToolAction` trait models synchronous, request–response
//! actions, and deferred execution requires task-spawning infrastructure
//! (a task store, polling loop, and cancellation handles) that is specific to
//! each tool binary's concurrency model. Tools that need background execution
//! should implement `ToolProvider` manually, overriding the three deferred
//! methods with their own task management. The default trait implementations
//! fall back to synchronous execution, so existing tools are unaffected.

use crate::ToolAction;
use async_trait::async_trait;
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolRagProfile, ToolSpec};

type SetCallContextHook = Box<dyn Fn(&str, Option<&str>) + Send + Sync>;
type SandboxHook = Box<dyn Fn(&SandboxConfigData) + Send + Sync>;
type PermissionHook = Box<dyn Fn(&str) + Send + Sync>;
type AllowPatternHook = Box<dyn Fn(&str, &str) + Send + Sync>;
type SetConfigHook = Box<dyn Fn(&serde_json::Value) + Send + Sync>;
type ConfigSchemaHook = Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>;

/// Shared hook storage used by both [`ActionSetProvider`] and
/// [`SingleActionProvider`]. Avoids duplicating the seven `Option<…Hook>`
/// fields and their `ToolProvider` implementations.
#[derive(Default)]
struct ProviderHooks {
    on_set_call_context: Option<SetCallContextHook>,
    on_sandbox: Option<SandboxHook>,
    on_approve_permission: Option<PermissionHook>,
    on_allow_pattern: Option<AllowPatternHook>,
    on_revoke_pattern: Option<AllowPatternHook>,
    on_set_config: Option<SetConfigHook>,
    config_schema: Option<ConfigSchemaHook>,
}

impl ProviderHooks {
    fn with_set_call_context_hook(
        mut self,
        hook: impl Fn(&str, Option<&str>) + Send + Sync + 'static,
    ) -> Self {
        self.on_set_call_context = Some(Box::new(hook));
        self
    }

    fn with_sandbox_hook(
        mut self,
        hook: impl Fn(&SandboxConfigData) + Send + Sync + 'static,
    ) -> Self {
        self.on_sandbox = Some(Box::new(hook));
        self
    }

    fn with_approve_permission_hook(mut self, hook: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_approve_permission = Some(Box::new(hook));
        self
    }

    fn with_allow_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.on_allow_pattern = Some(Box::new(hook));
        self
    }

    fn with_revoke_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.on_revoke_pattern = Some(Box::new(hook));
        self
    }

    fn with_set_config_hook(
        mut self,
        hook: impl Fn(&serde_json::Value) + Send + Sync + 'static,
    ) -> Self {
        self.on_set_config = Some(Box::new(hook));
        self
    }

    fn with_config_schema_hook(
        mut self,
        hook: impl Fn() -> Option<serde_json::Value> + Send + Sync + 'static,
    ) -> Self {
        self.config_schema = Some(Box::new(hook));
        self
    }

    fn call_set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        if let Some(hook) = &self.on_set_call_context {
            let turn_id = if ctx.turn_id.is_empty() {
                None
            } else {
                Some(ctx.turn_id.as_str())
            };
            hook(&ctx.conversation_id, turn_id);
        }
    }

    fn call_set_sandbox(&self, sandbox: &SandboxConfigData) {
        if let Some(hook) = &self.on_sandbox {
            hook(sandbox);
        }
    }

    fn call_approve_permission(&self, request_id: &str) {
        if let Some(hook) = &self.on_approve_permission {
            hook(request_id);
        }
    }

    fn call_allow_pattern(&self, action: &str, target_pattern: &str) {
        if let Some(hook) = &self.on_allow_pattern {
            hook(action, target_pattern);
        }
    }

    fn call_revoke_pattern(&self, action: &str, target_pattern: &str) {
        if let Some(hook) = &self.on_revoke_pattern {
            hook(action, target_pattern);
        }
    }

    fn call_set_config(&self, config: &serde_json::Value) {
        if let Some(hook) = &self.on_set_config {
            hook(config);
        }
    }

    fn call_config_schema(&self) -> Option<serde_json::Value> {
        self.config_schema.as_ref().and_then(|hook| hook())
    }
}

/// Adapts a flat `Vec<Box<dyn ToolAction>>` into a [`ToolProvider`].
///
/// `list_specs` and `call_tool` are dispatched generically over the action
/// list; lifecycle hooks (`set_call_context`, `set_sandbox`, permissions,
/// config) are no-ops unless registered via the corresponding `with_*`
/// builders.
pub struct ActionSetProvider {
    actions: Vec<Box<dyn ToolAction>>,
    hooks: ProviderHooks,
}

impl ActionSetProvider {
    /// Creates a provider dispatching over the given actions.
    #[must_use]
    pub fn new(actions: Vec<Box<dyn ToolAction>>) -> Self {
        Self {
            actions,
            hooks: ProviderHooks::default(),
        }
    }

    /// Registers a callback invoked on `ToolProvider::set_call_context`.
    #[must_use]
    pub fn with_set_call_context_hook(
        mut self,
        hook: impl Fn(&str, Option<&str>) + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_set_call_context_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::set_sandbox`.
    #[must_use]
    pub fn with_sandbox_hook(
        mut self,
        hook: impl Fn(&SandboxConfigData) + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_sandbox_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::approve_permission`.
    #[must_use]
    pub fn with_approve_permission_hook(
        mut self,
        hook: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_approve_permission_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::allow_pattern`.
    #[must_use]
    pub fn with_allow_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_allow_pattern_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::revoke_pattern`.
    #[must_use]
    pub fn with_revoke_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_revoke_pattern_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::set_config`.
    #[must_use]
    pub fn with_set_config_hook(
        mut self,
        hook: impl Fn(&serde_json::Value) + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_set_config_hook(hook);
        self
    }

    /// Registers a callback that returns the tool's config JSON Schema.
    #[must_use]
    pub fn with_config_schema_hook(
        mut self,
        hook: impl Fn() -> Option<serde_json::Value> + Send + Sync + 'static,
    ) -> Self {
        self.hooks = self.hooks.with_config_schema_hook(hook);
        self
    }
}

#[async_trait]
impl ToolProvider for ActionSetProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.actions.iter().map(|a| a.definition()).collect()
    }

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        self.actions.iter().map(|a| a.rag_profile()).collect()
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

    fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        self.hooks.call_set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.hooks.call_set_sandbox(sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        self.hooks.call_approve_permission(request_id);
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.hooks.call_allow_pattern(action, target_pattern);
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.hooks.call_revoke_pattern(action, target_pattern);
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.hooks.call_set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.hooks.call_config_schema()
    }
}

/// Adapts a single [`ToolAction`] into a [`ToolProvider`].
///
/// Thin wrapper over [`ActionSetProvider`] for tools that expose exactly
/// one action. Delegates all dispatch and lifecycle hook logic to the
/// inner `ActionSetProvider`, avoiding code duplication.
pub struct SingleActionProvider {
    inner: ActionSetProvider,
}

impl SingleActionProvider {
    /// Creates a provider for a single action.
    #[must_use]
    pub fn new(action: Box<dyn ToolAction>) -> Self {
        Self {
            inner: ActionSetProvider::new(vec![action]),
        }
    }

    /// Registers a callback invoked on `ToolProvider::set_call_context`.
    #[must_use]
    pub fn with_set_call_context_hook(
        mut self,
        hook: impl Fn(&str, Option<&str>) + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_set_call_context_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::set_sandbox`.
    #[must_use]
    pub fn with_sandbox_hook(
        mut self,
        hook: impl Fn(&SandboxConfigData) + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_sandbox_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::approve_permission`.
    #[must_use]
    pub fn with_approve_permission_hook(
        mut self,
        hook: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_approve_permission_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::allow_pattern`.
    #[must_use]
    pub fn with_allow_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_allow_pattern_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::revoke_pattern`.
    #[must_use]
    pub fn with_revoke_pattern_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_revoke_pattern_hook(hook);
        self
    }

    /// Registers a callback invoked on `ToolProvider::set_config`.
    #[must_use]
    pub fn with_set_config_hook(
        mut self,
        hook: impl Fn(&serde_json::Value) + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_set_config_hook(hook);
        self
    }

    /// Registers a callback that returns the tool's config JSON Schema.
    #[must_use]
    pub fn with_config_schema_hook(
        mut self,
        hook: impl Fn() -> Option<serde_json::Value> + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.with_config_schema_hook(hook);
        self
    }
}

#[async_trait]
impl ToolProvider for SingleActionProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        self.inner.list_rag_profiles()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        self.inner.set_call_context(ctx);
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

    fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.revoke_pattern(action, target_pattern);
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.inner.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.inner.config_schema()
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "provider registry lookup uses infallible test fixture"
)]
mod tests {
    use super::*;
    use ene_tool_proto::{ToolName, ToolRagProfile, ToolSpec};
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

        fn rag_profile(&self) -> ToolRagProfile {
            ToolRagProfile::from_tool_spec(&self.definition())
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
            .with_set_call_context_hook(move |_conv_id, _turn_id| {
                seen2.store(true, Ordering::SeqCst);
            });
        provider.set_call_context(&ene_tool_proto::CallContext {
            conversation_id: "abc".to_string(),
            turn_id: String::new(),
        });
        assert!(seen.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn action_set_provider_config_schema_delegates() {
        let provider = ActionSetProvider::new(vec![Box::new(EchoAction)])
            .with_config_schema_hook(|| {
                Some(serde_json::json!({"type": "object", "properties": {"threshold": {"type": "number"}}}))
            });
        let schema = provider.config_schema();
        assert!(schema.is_some());
        let s = schema.unwrap();
        assert!(
            s.get("properties")
                .and_then(|p| p.get("threshold"))
                .is_some()
        );
    }

    // ── SingleActionProvider tests ─────────────────────────────────

    #[tokio::test]
    async fn single_action_provider_dispatches_by_name() {
        let provider = SingleActionProvider::new(Box::new(EchoAction));
        let result = provider.call_tool("echo", "hi").await.unwrap();
        assert_eq!(result, "hi");
    }

    #[tokio::test]
    async fn single_action_provider_not_found_for_unknown_name() {
        let provider = SingleActionProvider::new(Box::new(EchoAction));
        let err = provider.call_tool("missing", "hi").await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[test]
    fn single_action_provider_lists_spec() {
        let provider = SingleActionProvider::new(Box::new(EchoAction));
        let specs = provider.list_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs.first().map(|spec| spec.name.as_str()), Some("echo"));
    }

    #[tokio::test]
    async fn single_action_provider_session_id_hook_runs() {
        let seen = Arc::new(AtomicBool::new(false));
        let seen2 = seen.clone();
        let provider = SingleActionProvider::new(Box::new(EchoAction)).with_set_call_context_hook(
            move |_conv_id, _turn_id| seen2.store(true, Ordering::SeqCst),
        );
        provider.set_call_context(&ene_tool_proto::CallContext {
            conversation_id: "abc".to_string(),
            turn_id: String::new(),
        });
        assert!(seen.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn single_action_provider_config_schema_delegates() {
        let provider =
            SingleActionProvider::new(Box::new(EchoAction)).with_config_schema_hook(|| {
                Some(serde_json::json!({"type": "object", "properties": {"x": {"type": "number"}}}))
            });
        let schema = provider.config_schema();
        assert!(schema.is_some());
        let s = schema.unwrap();
        assert!(s.get("properties").and_then(|p| p.get("x")).is_some());
    }
}
