//! Plugin trait definitions and streaming chunk types.
//!
//! Three independent traits — [`ToolPlugin`], [`LlmPlugin`], and
//! [`EmbedPlugin`] — let a plugin struct implement any subset; the server
//! dispatches requests to the appropriate trait based on the `PluginDispatch`
//! routing table.

use std::pin::Pin;

use async_trait::async_trait;
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, LlmProviderSpec, PluginError, SandboxConfigData,
    SttProviderSpec, TokenUsage, ToolError, ToolResult, ToolSpec, TtsProviderSpec,
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
    /// Token usage for the whole completion, set on the **final** chunk when
    /// the provider reports it. Intermediate chunks leave this `None`.
    pub usage: Option<TokenUsage>,
}

/// A completed (non-streaming) plugin chat response: text plus any token
/// usage the provider reported.
///
/// Returned by [`LlmPlugin::chat_completion`]; the plugin server maps it onto
/// [`PluginIpcResponse::ChatCompletionResult`](ene_plugin_proto::PluginIpcResponse).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginCompletion {
    /// The generated assistant text.
    pub text: String,
    /// Token usage reported by the provider, if any.
    pub usage: Option<TokenUsage>,
}

impl PluginCompletion {
    /// A completion with no usage information.
    #[must_use]
    pub fn text_only(text: String) -> Self {
        Self { text, usage: None }
    }
}

impl From<String> for PluginCompletion {
    /// Wrap a bare text response as a completion with no usage.
    fn from(text: String) -> Self {
        Self::text_only(text)
    }
}

/// A boxed, sendable stream of [`PluginStreamChunk`] results.
///
/// Returned by [`LlmPlugin::create_chat_stream`]. The plugin server iterates
/// this stream and writes one `StreamChunk` IPC response per item, followed
/// by a terminal `StreamEnd` or `StreamError`.
pub type PluginStream = Pin<Box<dyn Stream<Item = Result<PluginStreamChunk, PluginError>> + Send>>;

/// Capabilities advertised by a tool plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPluginCapabilities {
    /// Number of tools this plugin provides.
    pub tool_count: usize,
}

/// Plugin trait for tool execution.
///
/// Implement this trait to expose tools, deferred execution, permission
/// gating, and configuration. Every method has a sensible default so a
/// plugin can opt into exactly the capabilities it needs.
#[async_trait]
pub trait ToolPlugin: Send + Sync {
    /// Returns the tool capabilities advertised during the handshake.
    fn tool_capabilities(&self) -> ToolPluginCapabilities;

    /// Executes a tool by name with JSON-encoded arguments and an optional
    /// per-call context.
    async fn call_tool(
        &self,
        name: &str,
        _args: &str,
        _context: Option<&CallContext>,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    /// Executes a tool in deferred (background) mode.
    ///
    /// The default implementation runs synchronously via [`call_tool`](Self::call_tool).
    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&CallContext>,
    ) -> Result<DeferredOutcome, ToolError> {
        let result = self.call_tool(name, arguments, context).await?;
        Ok(DeferredOutcome::Sync(result))
    }

    /// Polls the status of a deferred (background) task by id.
    fn poll_deferred(&self, _task_id: &str) -> Result<DeferredStatus, ToolError> {
        Ok(DeferredStatus::Unknown)
    }

    /// Cancels a deferred (background) task by id.
    fn cancel_deferred(&self, _task_id: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Returns the tool specs this plugin exposes.
    fn list_tool_specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    /// Approves a pending destructive-operation permission request by ID.
    fn approve_permission(&self, _request_id: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Adds a session-wide permission allow pattern (action + target glob).
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Revokes a previously granted session-wide permission allow pattern.
    fn revoke_pattern(&self, _action: &str, _target_pattern: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Receives plugin-specific configuration (called once during Handshake).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Receives sandbox configuration (called once during Handshake).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Returns the JSON Schema for the configuration this plugin accepts.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Drain any pending deferred completion notifications.
    ///
    /// Returns a list of `(task_id, result)` pairs. The server calls this
    /// after each request to push completions to the host.
    fn drain_deferred_completions(&self) -> Vec<(String, Result<ToolResult, ToolError>)> {
        Vec::new()
    }
}

/// Plugin trait for LLM chat completions (streaming and non-streaming).
#[async_trait]
pub trait LlmPlugin: Send + Sync {
    /// Returns the LLM provider capabilities advertised during the handshake.
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec>;

    /// Creates a streaming chat completion.
    ///
    /// Returns a stream of [`PluginStreamChunk`] items. The default returns
    /// [`PluginError::NotSupported`] for plugins that do not provide LLM
    /// streaming.
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

    /// Performs a non-streaming chat completion.
    ///
    /// Returns a [`PluginCompletion`] carrying the assistant text plus any
    /// token usage the provider reported. The default returns
    /// [`PluginError::NotSupported`] for plugins that do not provide LLM
    /// completions.
    async fn chat_completion(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _model: String,
        _max_tokens: Option<u32>,
        _messages: Vec<serde_json::Value>,
        _json_schema: Option<serde_json::Value>,
    ) -> Result<PluginCompletion, PluginError> {
        Err(PluginError::not_supported("chat_completion"))
    }
}

/// Plugin trait for batch embedding computation.
#[async_trait]
pub trait EmbedPlugin: Send + Sync {
    /// Computes embeddings for a batch of text items.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide embeddings.
    ///
    /// # Item identity
    ///
    /// Items are passed as plain text (`Vec<String>`) rather than `(id, text)`
    /// pairs: any caller-side identifiers (e.g. `EmbeddingKind` or row ids) are
    /// intentionally dropped at the IPC boundary. Correlation is **positional**
    /// — the returned `Vec<Vec<f32>>` has exactly one vector per input item, in
    /// the same order. Callers must therefore keep their own id→index mapping
    /// and re-associate results by index. This keeps the wire format minimal;
    /// restore `(id, text)` only if a future provider needs per-item metadata.
    async fn embed_batch(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _model: String,
        _dimensions: Option<u32>,
        _items: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, PluginError> {
        Err(PluginError::not_supported("embed_batch"))
    }
}

/// Plugin trait for Text-to-Speech synthesis.
#[async_trait]
pub trait TtsPlugin: Send + Sync {
    /// Returns TTS capabilities advertised during the handshake.
    fn tts_capabilities(&self) -> Vec<TtsProviderSpec>;

    /// Synthesizes speech from text.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide TTS.
    async fn synthesize(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _text: String,
        _voice: String,
        _format: String,
    ) -> Result<Vec<u8>, PluginError> {
        Err(PluginError::not_supported("synthesize"))
    }
}

/// Plugin trait for Speech-to-Text transcription.
#[async_trait]
pub trait SttPlugin: Send + Sync {
    /// Returns STT capabilities advertised during the handshake.
    fn stt_capabilities(&self) -> Vec<SttProviderSpec>;

    /// Transcribes speech audio to text.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide STT.
    async fn transcribe(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _audio_data: Vec<u8>,
        _format: String,
    ) -> Result<String, PluginError> {
        Err(PluginError::not_supported("transcribe"))
    }
}
