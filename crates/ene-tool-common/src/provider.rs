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

/// Adapts a flat `Vec<Box<dyn ToolAction>>` into a [`ToolProvider`].
///
/// `list_specs` and `call_tool` are dispatched generically over the action
/// list; `set_session_id` / `set_sandbox` are no-ops unless a hook is
/// registered via [`with_session_id_hook`](Self::with_session_id_hook) /
/// [`with_sandbox_hook`](Self::with_sandbox_hook) — most mega-tools need one
/// or both to thread session/sandbox state into their actions' shared state.
pub struct ActionSetProvider {
    actions: Vec<Box<dyn ToolAction>>,
    on_session_id: Option<SessionIdHook>,
    on_sandbox: Option<SandboxHook>,
}

impl ActionSetProvider {
    /// Creates a provider dispatching over the given actions.
    #[must_use]
    pub fn new(actions: Vec<Box<dyn ToolAction>>) -> Self {
        Self {
            actions,
            on_session_id: None,
            on_sandbox: None,
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
}

/// Adapts a single [`ToolAction`] into a [`ToolProvider`].
///
/// Convenience wrapper for the individual-tool pattern (one binary, one
/// action) — equivalent to `ActionSetProvider::new(vec![Box::new(action)])`
/// but avoids the `Vec` boilerplate at the call site. Supports the same
/// session-id / sandbox hooks as [`ActionSetProvider`].
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_tool_proto::{KeywordSet, SideEffects, ToolCategory, ToolName, ToolSpec, ToolVersion};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct EchoAction;

    #[async_trait]
    impl ToolAction for EchoAction {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn definition(&self) -> ToolSpec {
            ToolSpec {
                name: ToolName::new("echo"),
                version: ToolVersion::new(1, 0, 0),
                display_name: "Echo".into(),
                summary: "Echoes the input".into(),
                description: "Echoes the input back unchanged.".into(),
                category: ToolCategory::Utility,
                keywords: KeywordSet::default(),
                parameters: serde_json::json!({"type": "object"}),
                examples: vec![],
                caveats: vec![],
                side_effects: SideEffects::ReadOnly,
                preconditions: vec![],
                related: vec![],
            }
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
