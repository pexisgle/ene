//! Compatibility adapter for wrapping legacy [`ToolProvider`] implementations
//! as [`ToolPlugin`](crate::ToolPlugin).
//!
//! [`ToolProviderPlugin`] bridges the deprecated `ToolProvider` surface
//! (from `ene-plugin-proto`) into the new `ToolPlugin` trait so that existing
//! tool binaries can be migrated incrementally.

use async_trait::async_trait;
use ene_plugin_proto::SandboxConfigData;
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, ToolError, ToolProvider, ToolSpec,
};

use crate::plugin::{ToolPlugin, ToolPluginCapabilities};

/// Wraps any [`ToolProvider`] into a [`ToolPlugin`].
///
/// # Example
///
/// ```rust,no_run
/// use ene_plugin::{ToolProviderPlugin, PluginDispatch, run_plugin_server};
/// # use ene_plugin_proto::ToolProvider;
///
/// # struct MyProvider;
/// # #[async_trait::async_trait]
/// # impl ToolProvider for MyProvider {
/// #     fn list_specs(&self) -> Vec<ene_plugin_proto::ToolSpec> { vec![] }
/// #     async fn call_tool(&self, _n: &str, _a: &str) -> Result<String, ene_plugin_proto::ToolError> { Ok(String::new()) }
/// # }
/// #[tokio::main]
/// async fn main() {
///     let provider = MyProvider;
///     let _ = run_plugin_server(PluginDispatch::new(
///         Some(std::sync::Arc::new(ToolProviderPlugin(provider))),
///         None,
///         None,
///     )).await;
/// }
/// ```
pub struct ToolProviderPlugin<T: ToolProvider>(pub T);

#[async_trait]
impl<T: ToolProvider + Send + Sync> ToolPlugin for ToolProviderPlugin<T> {
    fn tool_capabilities(&self) -> ToolPluginCapabilities {
        ToolPluginCapabilities {
            tool_count: self.0.list_specs().len(),
        }
    }

    fn list_tool_specs(&self) -> Vec<ToolSpec> {
        self.0.list_specs()
    }

    async fn call_tool(
        &self,
        name: &str,
        args: &str,
        context: Option<&CallContext>,
    ) -> Result<String, ToolError> {
        if let Some(ctx) = context {
            self.0.set_call_context(ctx);
        }
        self.0.call_tool(name, args).await
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&CallContext>,
    ) -> Result<DeferredOutcome, ToolError> {
        if let Some(ctx) = context {
            self.0.set_call_context(ctx);
        }
        self.0.call_tool_deferred(name, arguments).await
    }

    fn poll_deferred(&self, task_id: &str) -> Result<DeferredStatus, ToolError> {
        Ok(self.0.poll_deferred(task_id))
    }

    fn cancel_deferred(&self, task_id: &str) -> Result<(), ToolError> {
        self.0.cancel_deferred(task_id);
        Ok(())
    }

    fn approve_permission(&self, request_id: &str) -> Result<(), ToolError> {
        self.0.approve_permission(request_id);
        Ok(())
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) -> Result<(), ToolError> {
        self.0.allow_pattern(action, target_pattern);
        Ok(())
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) -> Result<(), ToolError> {
        self.0.revoke_pattern(action, target_pattern);
        Ok(())
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.0.set_sandbox(sandbox);
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.0.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.0.config_schema()
    }
}
