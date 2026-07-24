//! Compatibility adapter for wrapping `ToolProvider` implementations as plugins.
//!
//! [`ToolPluginAdapter`] bridges the legacy tool IPC surface (`ToolProvider`
//! from `ene-tool-proto`) into the unified [`Plugin`](crate::Plugin) trait so
//! existing tool binaries can be served via [`run_plugin_server`](crate::run_plugin_server)
//! without rewriting their internal dispatch logic.

use async_trait::async_trait;
use ene_plugin_proto::{PluginCapabilities, ToolError, ToolSpec};
use ene_plugin_proto::{SandboxConfigData, ToolProvider};

use crate::plugin::Plugin;

/// Wraps any [`ToolProvider`] into a [`Plugin`].
///
/// The adapter forwards `capabilities` (tool specs), `call_tool`,
/// `set_sandbox`, `set_config`, and `config_schema` to the inner provider.
/// All LLM/embedding methods use the default "not supported" implementations.
///
/// # Example
///
/// ```rust,no_run
/// use ene_plugin::{ToolPluginAdapter, run_plugin_server};
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
///     let _ = run_plugin_server(Box::new(ToolPluginAdapter(provider))).await;
/// }
/// ```
pub struct ToolPluginAdapter<T: ToolProvider>(pub T);

#[async_trait]
impl<T: ToolProvider> Plugin for ToolPluginAdapter<T> {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities {
            tools: self.0.list_specs(),
            ..Default::default()
        }
    }

    fn list_tool_specs(&self) -> Vec<ToolSpec> {
        self.0.list_specs()
    }

    async fn call_tool(&self, name: &str, args: &str) -> Result<String, ToolError> {
        self.0.call_tool(name, args).await
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
