//! `ToolProvider` adapters over collections of [`ToolAction`]s.
//!
//! Every hand-written `ToolProvider` in the `tools/` binaries re-implements
//! the same dispatch loop: match `call_tool`'s `name` against each action's
//! `name()`, forward to `execute`, and return [`ToolError::NotFound`] on a
//! miss. [`ActionSetProvider`] factors that loop
//! out so new tool binaries don't have to hand-write a `ToolProvider` impl
//! just to dispatch a fixed action list — see the ABI compatibility table in
//! `docs/tools/sdk.md` for how this fits into the wider tool ABI.

use crate::ToolAction;
use async_trait::async_trait;
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};

type SetCallContextHook = Box<dyn Fn(&str) + Send + Sync>;
type SandboxHook = Box<dyn Fn(&SandboxConfigData) + Send + Sync>;
type PermissionHook = Box<dyn Fn(&str) + Send + Sync>;
type AllowPatternHook = Box<dyn Fn(&str, &str) + Send + Sync>;
type SetConfigHook = Box<dyn Fn(&serde_json::Value) + Send + Sync>;
type ConfigSchemaHook = Box<dyn Fn() -> Option<serde_json::Value> + Send + Sync>;

/// Adapts a flat `Vec<Box<dyn ToolAction>>` into a [`ToolProvider`].
///
/// `list_specs` and `call_tool` are dispatched generically over the action
/// list; lifecycle hooks (`set_call_context`, `set_sandbox`, permissions,
/// config) are no-ops unless registered via the corresponding `with_*`
/// builders.
pub struct ActionSetProvider {
    actions: Vec<Box<dyn ToolAction>>,
    on_set_call_context: Option<SetCallContextHook>,
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
            on_set_call_context: None,
            on_sandbox: None,
            on_approve_permission: None,
            on_allow_pattern: None,
            on_set_config: None,
            config_schema: None,
        }
    }

    /// Registers a callback invoked on `ToolProvider::set_call_context`.
    #[must_use]
    pub fn with_set_call_context_hook(
        mut self,
        hook: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.on_set_call_context = Some(Box::new(hook));
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

    fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        if let Some(hook) = &self.on_set_call_context {
            hook(&ctx.conversation_id);
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
            .with_set_call_context_hook(move |_conv_id| seen2.store(true, Ordering::SeqCst));
        provider.set_call_context(&ene_tool_proto::CallContext {
            conversation_id: "abc".to_string(),
            turn_id: String::new(),
        });
        assert!(seen.load(Ordering::SeqCst));
    }
}
