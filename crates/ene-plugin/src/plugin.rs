//! Plugin trait definitions and streaming chunk types.
//!
//! Five independent traits — [`ToolPlugin`], [`LlmPlugin`], [`EmbedPlugin`],
//! [`TtsPlugin`], and [`SttPlugin`]. A plugin struct can implement any subset;
//! the server dispatches requests to the appropriate trait based on the
//! `PluginDispatch` routing table. Every trait inherits the shared
//! [`ConfigurablePlugin`] surface, so any plugin — tool or provider — can
//! receive its opaque configuration blob from the host and advertise a config
//! schema.

use std::pin::Pin;

use async_trait::async_trait;
use ene_plugin_proto::{
    CallContext, CapabilityRef, ConfigFieldError, ConfigOption, DeferredOutcome, DeferredStatus,
    LlmProviderSpec, PluginError, SandboxConfigData, SttProviderSpec, TokenUsage, ToolError,
    ToolResult, ToolSpec, TtsProviderSpec, VadEvent, VadProviderSpec,
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

/// A completed speech-to-text transcription.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginTranscription {
    /// Transcribed text.
    pub text: String,
    /// Detected language code (e.g. `"ja"`, `"en"`), when the plugin knows
    /// it. The whisper plugin reports the language hint it transcribed with.
    pub language: Option<String>,
}

/// Capabilities advertised by a tool plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPluginCapabilities {
    /// Number of tools this plugin provides.
    pub tool_count: usize,
}

// ── ConfigurablePlugin (shared by every plugin trait) ─────────────────

/// Shared configuration surface inherited by every plugin trait.
///
/// The host delivers plugin-specific configuration once during the IPC
/// handshake via [`set_config`](Self::set_config) (the `plugins.list.<name>.config`
/// blob) and [`set_profiles`](Self::set_profiles) (the
/// `plugins.list.<name>.profiles.<profile>` map). Both blobs are opaque to the
/// host — profile *selection* (e.g. per model/voice) is plugin-owned. A plugin
/// advertises the JSON Schema its config accepts via
/// [`config_schema`](Self::config_schema); fields it marks with
/// `x-ene-secret: true` are treated as secrets by the host (masked in logs,
/// redacted at the host boundary).
///
/// Dynamic config (protocol v5+) is opt-in: override the `supports_*` flags
/// and the corresponding handlers. Defaults keep older plugins on the static
/// schema path (host JSON Schema validation, no migration).
///
/// Every method has a default no-op implementation so a plugin opts into
/// exactly the configuration support it needs.
pub trait ConfigurablePlugin: Send + Sync {
    /// Receives plugin-specific configuration (called once during Handshake).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Receives per-profile plugin configuration (Handshake when
    /// `plugins.list.<name>.profiles` is set, and live `SetConfig`).
    ///
    /// The value is the raw `profiles` JSON object (`Map<profile, config>`);
    /// profile selection is plugin-owned. On live `SetConfig`, an empty object
    /// means profiles were cleared and must replace any previously stored map.
    fn set_profiles(&self, _profiles: &serde_json::Value) {}

    /// Returns the JSON Schema for the configuration this plugin accepts.
    ///
    /// Safe to call repeatedly — the host may re-fetch after a
    /// [`ConfigSchemaChanged`](ene_plugin_proto::PluginIpcResponse::ConfigSchemaChanged)
    /// push or whenever the UI needs a fresh schema.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Current config schema version this plugin expects (`0` = unversioned).
    fn config_version(&self) -> u32 {
        0
    }

    /// Advertise support for [`list_config_options`](Self::list_config_options).
    fn supports_list_config_options(&self) -> bool {
        false
    }

    /// Advertise support for [`validate_config`](Self::validate_config).
    fn supports_validate_config(&self) -> bool {
        false
    }

    /// Advertise support for [`migrate_config`](Self::migrate_config).
    fn supports_migrate_config(&self) -> bool {
        false
    }

    /// List dynamic options for a config path (e.g. `"voice"`).
    fn list_config_options(&self, _path: &str) -> Vec<ConfigOption> {
        Vec::new()
    }

    /// Validate a candidate config value; return field-level errors (empty = ok).
    fn validate_config(&self, _value: &serde_json::Value) -> Vec<ConfigFieldError> {
        Vec::new()
    }

    /// Migrate a stored config blob from `from_version` to the current version.
    ///
    /// Returns the migrated value. The server pairs it with
    /// [`config_version`](Self::config_version) in the IPC response.
    fn migrate_config(
        &self,
        _from_version: u32,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Ok(value)
    }

    /// Drain a pending schema-change notification for the host push path.
    ///
    /// Return `Some(schema)` once when the runtime schema has changed (e.g.
    /// after connecting to an external engine). The server pushes
    /// [`ConfigSchemaChanged`](ene_plugin_proto::PluginIpcResponse::ConfigSchemaChanged)
    /// and clears the pending state — subsequent drains return `None` until
    /// the next change.
    fn drain_config_schema_change(&self) -> Option<serde_json::Value> {
        None
    }
}

// ── ToolPlugin ──────────────────────────────────────────────────────────

/// Plugin trait for tool execution.
///
/// Implement this trait to expose tools, deferred execution, permission
/// gating, and configuration. Every method has a sensible default so a
/// plugin can opt into exactly the capabilities it needs.
#[async_trait]
pub trait ToolPlugin: ConfigurablePlugin + Send + Sync {
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

    /// Receives sandbox configuration (called once during Handshake).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

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
pub trait LlmPlugin: ConfigurablePlugin + Send + Sync {
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

// ── EmbedPlugin ─────────────────────────────────────────────────────────

/// Plugin trait for batch embedding computation.
#[async_trait]
pub trait EmbedPlugin: ConfigurablePlugin + Send + Sync {
    /// Provider kinds this plugin serves batch embeddings for.
    ///
    /// Advertised in the handshake `PluginCapabilities.embed_providers` so
    /// the host can register an embedding factory per kind. The default
    /// returns an empty list — a plugin that only implements `embed_batch`
    /// without advertising a kind is never routed to.
    fn embed_providers(&self) -> Vec<String> {
        Vec::new()
    }

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

// ── CapabilityProvider ──────────────────────────────────────────────────

/// Plugin trait for serving mediated capability calls from other plugins.
///
/// A plugin that declares capabilities in its `provides` list (via
/// `#[provider(provides = "...")]`) implements this trait to serve them: the
/// host routes a consumer's [`CapabilityCall`](ene_plugin_proto::CapabilityCall)
/// here after resolving and authenticating it. The default returns
/// [`PluginError::NotSupported`] for plugins that do not serve capability
/// calls; the plugin server additionally refuses calls for capabilities the
/// plugin did not declare, so a binary never serves undeclared capabilities.
#[async_trait]
pub trait CapabilityProvider: ConfigurablePlugin + Send + Sync {
    /// Executes one capability method call.
    ///
    /// `capability` is the requested reference (`gguf-runner@1`) and `method`
    /// / `payload` follow that capability's published contract (e.g.
    /// `generate` with `{ model, prompt, json_schema? }`). The response is a
    /// method-defined JSON value, opaque to the host.
    async fn call_capability(
        &self,
        capability: &CapabilityRef,
        method: &str,
        _payload: serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        Err(PluginError::not_supported(format!(
            "capability call {capability}/{method}"
        )))
    }
}

// ── TtsPlugin ───────────────────────────────────────────────────────────

/// Plugin trait for Text-to-Speech synthesis.
#[async_trait]
pub trait TtsPlugin: ConfigurablePlugin + Send + Sync {
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
pub trait SttPlugin: ConfigurablePlugin + Send + Sync {
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
    ) -> Result<PluginTranscription, PluginError> {
        Err(PluginError::not_supported("transcribe"))
    }
}

// ── VadPlugin ───────────────────────────────────────────────────────────

/// Plugin trait for voice activity detection.
///
/// VAD is stateful per session: the host generates a unique `session_id`
/// per engine instance and streams fixed-size PCM chunks to it, one
/// [`process_chunk`](Self::process_chunk) call per chunk. `reset` discards
/// the session's state, mirroring `ene_ai::VadEngine::reset`. The trait is
/// `&self` like the other plugin traits, so implementations keep per-session
/// engine state behind a mutex keyed by `session_id`.
#[async_trait]
pub trait VadPlugin: ConfigurablePlugin + Send + Sync {
    /// Returns VAD capabilities advertised during the handshake.
    fn vad_capabilities(&self) -> Vec<VadProviderSpec>;

    /// Processes one PCM chunk (or resets a session) and returns the
    /// resulting voice activity event.
    ///
    /// The default returns [`PluginError::NotSupported`] for plugins that
    /// do not provide VAD.
    async fn process_chunk(
        &self,
        _kind: &str,
        _config: serde_json::Value,
        _session_id: String,
        _pcm: Vec<f32>,
        _reset: bool,
    ) -> Result<VadEvent, PluginError> {
        Err(PluginError::not_supported("process_chunk"))
    }
}
