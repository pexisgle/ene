use crate::action;
use async_trait::async_trait;
use ene_plugin::ActionSetProvider;
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};

/// Built-in provider for the `__NAMESPACE__` tool namespace.
///
/// Wraps [`ActionSetProvider`] so stateful growth (DB socket via the
/// `set_sandbox` hook, session context, permission approval hooks, shared
/// state) can be added without restructuring `main`. When adding hooks,
/// forward the matching `ToolProvider` lifecycle methods (`set_sandbox`,
/// `set_call_context`, `approve_permission`, ...) to `self.inner`, or the
/// hooks never fire.
pub struct __PROVIDER_NAME__ {
    inner: ActionSetProvider,
}

impl __PROVIDER_NAME__ {
    #[must_use]
    pub fn new() -> Self {
        let actions: Vec<Box<dyn ene_plugin::ToolAction>> =
            vec![Box::new(action::EchoAction::default())];
        Self {
            inner: ActionSetProvider::new(actions),
        }
    }
}

impl Default for __PROVIDER_NAME__ {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for __PROVIDER_NAME__ {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }
}
