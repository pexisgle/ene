//! Plugin trait definitions and streaming chunk types.
//!
//! Three independent traits replace the old monolithic [`Plugin`] (now removed):
//! [`ToolPlugin`], [`LlmPlugin`], and [`EmbedPlugin`]. A plugin struct can
//! implement any subset; the server dispatches requests to the appropriate
//! trait based on the `PluginDispatch` routing table.

use std::pin::Pin;

use async_trait::async_trait;
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, LlmProviderSpec, PluginError, SandboxConfigData,
    SttProviderSpec, ToolError, ToolResult, ToolSpec, TtsProviderSpec,
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
/// Returned by [`LlmPlugin::create_chat_stream`]. The plugin server iterates
/// this stream and writes one `StreamChunk` IPC response per item, followed
/// by a terminal `StreamEnd` or `StreamError`.
pub type PluginStream = Pin<Box<dyn Stream<Item = Result<PluginStreamChunk, PluginError>> + Send>>;

/// Capabilities advertised by a tool plugin.
pub struct ToolPluginCapabilities {
    /// Number of tools this plugin provides.
    pub tool_count: usize,
}

// ── ToolPlugin ──────────────────────────────────────────────────────────

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

// ── LlmPlugin ──────────────────────────────────────────────────────────

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
}

// ── EmbedPlugin ─────────────────────────────────────────────────────────

/// Plugin trait for batch embedding computation.
#[async_trait]
pub trait EmbedPlugin: Send + Sync {
    /// Computes embeddings for a batch of text items.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide embeddings.
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

// ── TtsPlugin ───────────────────────────────────────────────────────────

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

// ── SttPlugin ───────────────────────────────────────────────────────────

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
