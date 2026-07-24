//! Plugin trait and streaming chunk types.
//!
//! The [`Plugin`] trait is the authoring interface for plugin binaries. A
//! plugin can expose tools, LLM providers (streaming and non-streaming),
//! and embedding providers simultaneously — the [`PluginCapabilities`]
//! advertised during the handshake tells the host which registries to
//! populate.

use std::pin::Pin;

use async_trait::async_trait;
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, PluginCapabilities, PluginError,
    SandboxConfigData, ToolError, ToolSpec,
};
use tokio_stream::Stream;

/// A single chunk from a streaming LLM response.
///
/// Each chunk carries incremental text and/or tool-call deltas. The plugin
/// server translates these into [`PluginIpcResponse::StreamChunk`](ene_plugin_proto::PluginIpcResponse)
/// messages on the wire.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginStreamChunk {
    /// Incremental text content (`None` when only tool calls advance).
    pub text_delta: Option<String>,
    /// Incremental tool-call JSON deltas (partial function-call arguments).
    pub tool_calls_delta: Option<Vec<serde_json::Value>>,
}

/// A boxed, sendable stream of [`PluginStreamChunk`] results.
///
/// Returned by [`Plugin::create_chat_stream`]. The plugin server iterates
/// this stream and writes one `StreamChunk` IPC response per item, followed
/// by a terminal `StreamEnd` or `StreamError`.
pub type PluginStream = Pin<Box<dyn Stream<Item = Result<PluginStreamChunk, PluginError>> + Send>>;

/// The plugin authoring trait.
///
/// Implement this trait to create a plugin binary. Only [`capabilities`](Self::capabilities)
/// is required; every other method has a sensible default (returning
/// "not supported" or "not found") so a plugin can opt into exactly the
/// capabilities it provides.
///
/// # Setter-call ordering contract
///
/// During the IPC handshake the server calls [`set_sandbox`](Self::set_sandbox)
/// and [`set_config`](Self::set_config) **once**, synchronously, before the
/// first provider or tool call on the same connection. Because each
/// connection is spawned as a separate Tokio task, interior mutability
/// (e.g. `RwLock`) is required for any state these setters write.
///
/// # Example
///
/// ```rust,no_run
/// use async_trait::async_trait;
/// use ene_plugin::{Plugin, PluginCapabilities, PluginError, PluginStream};
///
/// struct MyPlugin;
///
/// #[async_trait]
/// impl Plugin for MyPlugin {
///     fn capabilities(&self) -> PluginCapabilities {
///         PluginCapabilities::default()
///     }
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<(), PluginError> {
///     ene_plugin::run_plugin_server(Box::new(MyPlugin)).await
/// }
/// ```
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Returns the capabilities this plugin advertises during the handshake.
    fn capabilities(&self) -> PluginCapabilities;

    // ── Tool support (optional) ──

    /// Executes a tool by name with JSON-encoded arguments.
    ///
    /// The default returns [`ToolError::NotFound`] for plugins that do not
    /// expose tools.
    async fn call_tool(&self, name: &str, _args: &str) -> Result<String, ToolError> {
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    /// Executes a tool in deferred (background) mode.
    ///
    /// A background-capable plugin should start the work asynchronously and
    /// return [`DeferredOutcome::Deferred`] with a unique `task_id`; the
    /// completion is delivered later out-of-band. Plugins that do not support
    /// deferral keep the default implementation, which runs the call
    /// synchronously via [`call_tool`](Self::call_tool) and returns
    /// [`DeferredOutcome::Sync`].
    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredOutcome, ToolError> {
        Ok(DeferredOutcome::Sync(
            self.call_tool(name, arguments).await?,
        ))
    }

    /// Polls the status of a deferred (background) task by id.
    ///
    /// The default returns [`DeferredStatus::Unknown`] for plugins that do
    /// not support deferral.
    fn poll_deferred(&self, _task_id: &str) -> Result<DeferredStatus, ToolError> {
        Ok(DeferredStatus::Unknown)
    }

    /// Cancels a deferred (background) task by id.
    ///
    /// The default is a no-op for plugins that do not support deferral.
    fn cancel_deferred(&self, _task_id: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Sets the call context (conversation + turn identifiers).
    ///
    /// Plugins that want session-scoped state (e.g. undo checkpoints)
    /// should override this method. The default is a no-op.
    fn set_call_context(&self, _ctx: &CallContext) -> Result<(), ToolError> {
        Ok(())
    }

    /// Approves a pending destructive-operation permission request by ID.
    ///
    /// The default is a no-op for plugins that do not gate on permissions.
    fn approve_permission(&self, _request_id: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Adds a session-wide permission allow pattern (action + target glob).
    ///
    /// The default is a no-op for plugins that do not gate on permissions.
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Revokes a previously granted session-wide permission allow pattern.
    ///
    /// The default is a no-op for plugins that do not gate on permissions.
    fn revoke_pattern(&self, _action: &str, _target_pattern: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Returns the tool specs this plugin exposes.
    ///
    /// The default delegates to [`capabilities`](Self::capabilities).
    /// Override if constructing the full capabilities struct is expensive.
    fn list_tool_specs(&self) -> Vec<ToolSpec> {
        self.capabilities().tools
    }

    // ── LLM streaming (optional) ──

    /// Creates a streaming chat completion.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide LLM streaming.
    async fn create_chat_stream(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _model: String,
        _max_tokens: Option<u32>,
        _messages: Vec<serde_json::Value>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<PluginStream, PluginError> {
        Err(PluginError::not_supported("create_chat_stream"))
    }

    // ── LLM completion (optional) ──

    /// Performs a non-streaming chat completion.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide LLM completions.
    async fn chat_completion(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _model: String,
        _max_tokens: Option<u32>,
        _messages: Vec<serde_json::Value>,
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, PluginError> {
        Err(PluginError::not_supported("chat_completion"))
    }

    // ── Embedding (optional) ──

    /// Computes embeddings for a batch of items.
    ///
    /// Each item is a `(id, text)` pair. The default returns
    /// [`PluginError::NotSupported`] for plugins that do not provide
    /// embeddings.
    async fn embed_batch(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _model: String,
        _dimensions: Option<usize>,
        _items: Vec<(String, String)>,
    ) -> Result<Vec<Vec<f32>>, PluginError> {
        Err(PluginError::not_supported("embed_batch"))
    }

    // ── Configuration ──

    /// Receives plugin-specific configuration (called once during Handshake).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Receives sandbox configuration (called once during Handshake).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Returns the JSON Schema for the configuration this plugin accepts.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
