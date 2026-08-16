//! Compatibility adapter for wrapping legacy [`ToolProvider`] implementations
//! as [`ToolPlugin`](crate::ToolPlugin).
//!
//! [`ToolProviderPlugin`] bridges the deprecated `ToolProvider` surface
//! (from `ene-plugin-proto`) into the new `ToolPlugin` trait so that existing
//! tool binaries can be migrated incrementally.

use async_trait::async_trait;
use ene_plugin_proto::SandboxConfigData;
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, ToolError, ToolProvider, ToolResult, ToolSpec,
};
use tokio::sync::Mutex;

use crate::plugin::{ConfigurablePlugin, ToolPlugin, ToolPluginCapabilities};

/// Wraps any [`ToolProvider`] into a [`ToolPlugin`].
///
/// Serializes `set_call_context` + tool call to prevent interleaved context
/// from concurrent connections through the legacy [`ToolProvider`] shared-`&self`
/// surface.
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
///         Some(std::sync::Arc::new(ToolProviderPlugin::new(provider))),
///         None,
///         None,
///         None,
///         None,
///     )).await;
/// }
/// ```
pub struct ToolProviderPlugin<T: ToolProvider> {
    pub(crate) provider: T,
    /// Serializes `set_call_context` + tool call pairs so that concurrent
    /// connections through the shared-`&self` legacy path cannot interleave
    /// their context writes. This is a `tokio::sync::Mutex` because the guard
    /// is intentionally held across `.await` boundaries while the tool call
    /// runs; unlike `parking_lot::Mutex`, its `lock().await` yields the worker
    /// thread instead of blocking it, so a suspended call cannot deadlock a
    /// runtime thread that another task needs to acquire the same lock.
    call_mutex: Mutex<()>,
}

impl<T: ToolProvider> ToolProviderPlugin<T> {
    pub fn new(provider: T) -> Self {
        Self {
            provider,
            call_mutex: Mutex::new(()),
        }
    }
}

#[async_trait]
impl<T: ToolProvider + Send + Sync> ToolPlugin for ToolProviderPlugin<T> {
    fn tool_capabilities(&self) -> ToolPluginCapabilities {
        ToolPluginCapabilities {
            tool_count: self.provider.list_specs().len(),
        }
    }

    fn list_tool_specs(&self) -> Vec<ToolSpec> {
        self.provider.list_specs()
    }

    async fn call_tool(
        &self,
        name: &str,
        args: &str,
        context: Option<&CallContext>,
    ) -> Result<ToolResult, ToolError> {
        let _lock = self.call_mutex.lock().await;
        if let Some(ctx) = context {
            self.provider.set_call_context(ctx);
        }
        self.provider
            .call_tool(name, args)
            .await
            .map(ToolResult::text)
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&CallContext>,
    ) -> Result<DeferredOutcome, ToolError> {
        let _lock = self.call_mutex.lock().await;
        if let Some(ctx) = context {
            self.provider.set_call_context(ctx);
        }
        let outcome = self.provider.call_tool_deferred(name, arguments).await?;
        Ok(match outcome {
            DeferredOutcome::Sync(result) => DeferredOutcome::Sync(result),
            DeferredOutcome::Deferred { task_id } => DeferredOutcome::Deferred { task_id },
        })
    }

    fn poll_deferred(&self, task_id: &str) -> Result<DeferredStatus, ToolError> {
        Ok(self.provider.poll_deferred(task_id))
    }

    fn cancel_deferred(&self, task_id: &str) -> Result<(), ToolError> {
        self.provider.cancel_deferred(task_id);
        Ok(())
    }

    fn approve_permission(&self, request_id: &str) -> Result<(), ToolError> {
        self.provider.approve_permission(request_id);
        Ok(())
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) -> Result<(), ToolError> {
        self.provider.allow_pattern(action, target_pattern);
        Ok(())
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) -> Result<(), ToolError> {
        self.provider.revoke_pattern(action, target_pattern);
        Ok(())
    }
}

impl<T: ToolProvider> ConfigurablePlugin for ToolProviderPlugin<T> {
    fn set_config(&self, config: &serde_json::Value) {
        self.provider.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.provider.config_schema()
    }

    // `set_sandbox`/`set_config` are synchronous trait methods invoked exactly
    // once during the handshake, before any tool call on the connection. They
    // cannot await the async `call_mutex`, so they are not serialized under it.
    // Last-writer-wins is acceptable here: the wrapped [`ToolProvider`]
    // contractually requires interior mutability (e.g. `RwLock`) for any state
    // these setters write, and handshakes do not overlap with tool calls on the
    // same connection.
    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.provider.set_sandbox(sandbox);
    }
}
