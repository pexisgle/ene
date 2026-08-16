use crate::action;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};

/// Every action is pure: it reads no state and mutates nothing, so the
/// provider needs no config or sandbox hooks.
pub struct RandomToolProvider {
    inner: ActionSetProvider,
}

impl RandomToolProvider {
    #[must_use]
    pub fn new() -> Self {
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::NumberAction::default()),
            Box::new(action::UuidAction::default()),
            Box::new(action::PickAction::default()),
            Box::new(action::ColorAction::default()),
        ];

        Self {
            inner: ActionSetProvider::new(actions),
        }
    }
}

impl Default for RandomToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for RandomToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &ene_plugin_proto::SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.inner.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.inner.config_schema()
    }
}
