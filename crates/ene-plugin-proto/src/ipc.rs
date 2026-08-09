//! Plugin IPC wire protocol (protocol version 8).
//!
//! Extends the tool IPC (v2) with streaming LLM messages and a richer
//! handshake that carries [`PluginCapabilities`]. The framing is identical:
//! a 4-byte little-endian length prefix followed by a payload. The handshake
//! exchange always uses JSON; every frame after the handshake uses the
//! negotiated [`WireFormat`] (`MessagePack` for protocol v6+, JSON for older
//! versions — see [`WireFormat::for_version`]).

use crate::capabilities::PluginCapabilities;
use crate::capability_service::{CapabilityCall, CapabilityCallResult};
use crate::error::PluginError;
use crate::sandbox::SandboxConfigData;
use crate::tool_error::ToolError;
use crate::tool_ipc::{CallContext, DeferredStatus};
use crate::tool_types::{ToolResult, ToolSpec};
use crate::usage::TokenUsage;
use crate::wire::WireFormat;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum allowed IPC message size in bytes (64 MB).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Negotiated IPC protocol version range.
///
/// The host sends the range it supports; the plugin responds with the range
/// it supports. The negotiated version is the highest number that falls within
/// both ranges, chosen by the plugin and reported in the ack. If the ranges do
/// not overlap, the handshake must fail.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionRange {
    /// Minimum supported protocol version (inclusive).
    pub min: u32,
    /// Maximum supported protocol version (inclusive).
    pub max: u32,
}

impl VersionRange {
    /// Returns the highest protocol version supported by both `self` and
    /// `other` (i.e. `min(max_a, max_b)`), or `None` when the ranges do not
    /// overlap.
    ///
    /// Choosing the highest common version lets a newer peer take advantage of
    /// its latest features whenever the other side also supports them, while
    /// still agreeing on a lower version for partial overlap.
    pub fn negotiate(&self, other: &VersionRange) -> Option<u32> {
        let highest_common = self.max.min(other.max);
        let lowest_common = self.min.max(other.min);
        (highest_common >= lowest_common).then_some(highest_common)
    }

    /// Returns whether `version` falls within this range (inclusive).
    pub const fn contains(&self, version: u32) -> bool {
        version >= self.min && version <= self.max
    }

    /// Returns the range of protocol versions the host advertises during the
    /// handshake: `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]`.
    ///
    /// The host is the side responsible for maintaining backward
    /// compatibility (N-1 support policy — see the module docs), so it is
    /// the only side that should construct a multi-version range. A plugin
    /// binary should keep declaring the single version it was built against
    /// via `VersionRange { min: N, max: N }`; this concentrates the
    /// compatibility burden in the host rather than pushing it onto every
    /// plugin author.
    pub const fn host_supported() -> Self {
        Self {
            min: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
            max: PLUGIN_IPC_PROTOCOL_VERSION,
        }
    }
}

/// Plugin IPC protocol version.
///
/// v8 adds the **Broker channel**: `HostServiceId` gains the `Artifact`,
/// `File`, `Network`, `Process`, and `Platform` passengers (see
/// [`crate::broker`]) and `SandboxConfigData` carries the plugin's broker
/// socket and per-plugin temp directory. Hosts gate broker traffic on
/// `negotiated_version >= 8`; v7 plugins keep the `db` / `capability`
/// passengers only.
///
/// v7 adds:
/// - `ProcessVadChunk` / `VadChunkResult` for out-of-process voice activity
///   detection, and `PluginCapabilities.vad_providers` (`VadProviderSpec`).
///   Hosts gate `ProcessVadChunk` on `negotiated_version >= 7` (see the
///   `supports_vad()` gate in `ene-plugin-host`), so v6 plugin binaries never
///   receive the new request variant.
///
/// v6 changes the wire format: after the JSON handshake exchange, every
/// frame is `MessagePack` when both sides negotiated v6 (see
/// [`WireFormat`]). Peers that negotiated v5 or lower keep the original
/// JSON framing, so N-1 backward compatibility is preserved without any
/// per-frame format tag.
///
/// v5 extends v4 with:
/// - `SetConfig` / `ConfigApplied` for pushing updated plugin configuration
///   to a live plugin without restarting it
/// - Dynamic config surface (same protocol generation; requires rebuilt
///   plugins): `ListConfigOptions`, `ValidateConfig`, `MigrateConfig`, and
///   the `ConfigSchemaChanged` push. Hosts gate these on
///   `negotiated_version >= 5` **and** the matching
///   [`PluginCapabilities`](crate::PluginCapabilities) flags (serde-default
///   `false` on older v5 binaries that lack the variants).
///
/// v4 extends v3 with:
/// - `CancelStream` for explicit stream cancellation (from host to plugin)
/// - `DeferredCompleted` for push-based deferred task notification
/// - `ToolResult` structured return type (replaces opaque `String`)
///
/// v3 extends the tool IPC v2 with:
/// - `Handshake` gains `plugin_config` (replaces `tool_config` for plugins)
/// - `HandshakeAck` gains `capabilities: PluginCapabilities`
/// - Streaming LLM messages (`CreateChatStream`, `StreamChunk`, `StreamEnd`,
///   `StreamError`)
/// - `ChatCompletion` / `EmbedBatch` for non-streaming provider calls
///
/// post-v3 (no version bump): `Handshake` gains `plugin_profiles`
/// (per-profile plugin configuration, delivered alongside `plugin_config`).
/// The field is `#[serde(default)]`, so older peers stay wire-compatible
/// without a version bump.
///
/// ## Versioning policy (N-1 backward compatibility)
///
/// Plugins ship as separate out-of-process binaries (`plugins/tool/*`,
/// `plugins/provider/*`), so bumping this constant does not recompile
/// already-installed plugin binaries. The host therefore maintains
/// **one version of backward compatibility**: it always advertises
/// `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]` during
/// the handshake (see [`VersionRange::host_supported`]) rather than a single
/// pinned value, so a plugin built against the previous protocol version can
/// still connect. A plugin binary is *not* required to support a range; it
/// may keep declaring `VersionRange { min: N, max: N }` for whatever version
/// it was built against.
///
/// When bumping `PLUGIN_IPC_PROTOCOL_VERSION`, also bump
/// [`PLUGIN_IPC_MIN_SUPPORTED_VERSION`] by the same amount, which drops
/// support for the oldest previously-supported version. A version bump is
/// only needed for: changing the meaning of an existing message, adding a
/// required field, or removing/renaming an enum variant. New fields should
/// use `#[serde(default)]` so older/newer peers stay wire-compatible without
/// a version bump.
pub const PLUGIN_IPC_PROTOCOL_VERSION: u32 = 8;

/// The oldest plugin IPC protocol version the host still accepts.
///
/// Always `PLUGIN_IPC_PROTOCOL_VERSION - 1` (N-1 support policy — see the
/// [`PLUGIN_IPC_PROTOCOL_VERSION`] docs). A plugin binary built against this
/// version can still complete the handshake and connect, even though the
/// host has moved on to a newer protocol version.
pub const PLUGIN_IPC_MIN_SUPPORTED_VERSION: u32 = PLUGIN_IPC_PROTOCOL_VERSION - 1;

/// Default audio format used when none is specified.
fn default_audio_format() -> String {
    "wav".to_string()
}

/// One selectable option for a dynamic config field (e.g. a voice list).
///
/// Returned by [`PluginIpcRequest::ListConfigOptions`]. `value` is what the
/// host writes into the config blob; `label` is UI-facing; `group` optionally
/// buckets options (e.g. VOICEVOX speaker style families).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigOption {
    /// Value written into the config when this option is selected.
    pub value: serde_json::Value,
    /// Human-readable label for UI presentation.
    pub label: String,
    /// Optional grouping key for UI sections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A field-level validation error returned by
/// [`PluginIpcRequest::ValidateConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigFieldError {
    /// JSON-pointer-style path to the offending field (e.g. `"mmproj_path"`).
    pub field_path: String,
    /// Human-readable error message suitable for form display.
    pub message: String,
}

/// Plugin IPC request — host → plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginIpcRequest {
    /// Handshake to negotiate protocol version, exchange sandbox config,
    /// and push plugin-specific configuration. The host sends the version
    /// range it supports; the plugin negotiates and reports the chosen version
    /// in the ack.
    Handshake {
        /// Host's supported protocol version range.
        version: VersionRange,
        /// Sandbox configuration to apply.
        #[serde(default)]
        sandbox: SandboxConfigData,
        /// Plugin-specific configuration JSON.
        plugin_config: Option<serde_json::Value>,
        /// Per-profile plugin configuration JSON (`plugins.list.<name>.profiles`).
        ///
        /// Opaque to the host: profile selection is plugin-owned (a single
        /// plugin can need different settings per model/profile). Absent for
        /// older hosts; `#[serde(default)]` keeps the wire ABI stable.
        #[serde(default)]
        plugin_profiles: Option<serde_json::Value>,
    },
    /// Graceful shutdown.
    Shutdown,
    /// Liveness probe. The plugin must reply with [`PluginIpcResponse::Pong`].
    ///
    /// Carries a `request_id` so the host's single reader task can correlate
    /// the `Pong` with the originating probe. Older plugins that omit
    /// the field still interoperate because it is `#[serde(default)]`.
    Ping {
        /// Unique request identifier for correlating the `Pong`.
        #[serde(default)]
        request_id: String,
    },
    /// Request the plugin's config JSON Schema.
    GetConfigSchema {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
    },
    /// List all available tool specs.
    ListTools {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
    },
    /// Execute a tool by name with JSON arguments.
    CallTool {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Tool name to call.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
        /// When `true`, request deferred (background) execution.
        #[serde(default)]
        deferred: bool,
        /// Per-call context (conversation + turn identifiers).
        ///
        /// Supersedes the deprecated `SetCallContext` message. When present,
        /// the context applies to this single tool call only and does not
        /// persist for subsequent calls.
        #[serde(default)]
        context: Option<CallContext>,
    },
    /// Set the call context (conversation + turn identifiers).
    SetCallContext {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Conversation-level identifier.
        conversation_id: String,
        /// Turn-level identifier within the conversation.
        turn_id: String,
    },
    /// Poll the status of a deferred (background) task by id.
    PollDeferred {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// The `task_id` returned by a prior [`PluginIpcResponse::DeferredAccepted`].
        task_id: String,
    },
    /// Cancel an in-progress chat stream.
    CancelStream {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// The `request_id` of the `CreateChatStream` to cancel.
        stream_request_id: String,
    },
    /// Push updated plugin configuration to a live plugin (protocol v5+).
    ///
    /// The plugin applies the blob via `ConfigurablePlugin::set_config` (and
    /// `set_profiles`) and replies with
    /// [`PluginIpcResponse::ConfigApplied`]. Every peer in the host's N-1
    /// window (v5+) knows this variant, so the live push always applies.
    SetConfig {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Plugin-specific configuration JSON (same payload as Handshake
        /// `plugin_config`).
        config: serde_json::Value,
        /// Per-profile plugin configuration JSON.
        ///
        /// `None` means profiles were cleared on the host and the live plugin
        /// must replace any previously stored map (typically with `{}`).
        #[serde(default)]
        profiles: Option<serde_json::Value>,
    },
    /// List dynamic options for a config path (protocol v5+, capability-gated).
    ///
    /// Used when enum values cannot be baked into a static JSON Schema
    /// (runtime engine speakers, remote voice catalogs, etc.). The host
    /// sends this only when
    /// [`PluginCapabilities::supports_list_config_options`] is set.
    ListConfigOptions {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Dot/JSON-pointer style path within the plugin config
        /// (e.g. `"voice"` or `"speaker.id"`).
        path: String,
    },
    /// Ask the plugin to validate a config value (protocol v5+, capability-gated).
    ///
    /// Covers cross-field rules JSON Schema cannot express. The host sends
    /// this only when [`PluginCapabilities::supports_validate_config`] is set;
    /// otherwise it falls back to local JSON Schema validation.
    ValidateConfig {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Candidate configuration JSON to validate.
        value: serde_json::Value,
    },
    /// Ask the plugin to migrate a stored config blob (protocol v5+,
    /// capability-gated).
    ///
    /// Sent when the host's stored `config_version` is older than the
    /// plugin's advertised [`PluginCapabilities::config_version`]. The host
    /// sends this only when [`PluginCapabilities::supports_migrate_config`]
    /// is set; otherwise migration is skipped.
    MigrateConfig {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Version the stored blob was written under.
        from_version: u32,
        /// Stored configuration JSON to migrate.
        value: serde_json::Value,
    },
    /// Cancel a deferred (background) task by id.
    CancelDeferred {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// The `task_id` of the background task to cancel.
        task_id: String,
    },
    /// Approve a pending permission request.
    ApprovePermission {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// ID of the permission request to approve.
        #[serde(default)]
        permission_request_id: String,
    },
    /// Register a session-wide permission allow pattern.
    AllowPattern {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Action pattern (e.g. `"filesystem_write"`).
        action: String,
        /// Target glob pattern.
        target_pattern: String,
    },
    /// Revoke a previously granted session-wide permission allow pattern.
    RevokePattern {
        /// Unique request identifier for correlating the response.
        #[serde(default)]
        request_id: String,
        /// Action pattern to revoke.
        action: String,
        /// Target glob pattern to revoke.
        target_pattern: String,
    },
    /// Create a streaming chat completion.
    ///
    /// The plugin responds with N × [`PluginIpcResponse::StreamChunk`]
    /// followed by a terminal [`PluginIpcResponse::StreamEnd`] or
    /// [`PluginIpcResponse::StreamError`], all carrying the same `request_id`.
    CreateChatStream {
        /// Unique request identifier for correlating stream responses.
        request_id: String,
        /// Provider kind (e.g. `"anthropic"`).
        provider_kind: String,
        /// Provider-specific configuration JSON (API keys, base URL, etc.).
        provider_config: serde_json::Value,
        /// Model identifier.
        model: String,
        /// Maximum tokens to generate.
        max_tokens: Option<u32>,
        /// Chat messages in provider-agnostic JSON format.
        messages: Vec<serde_json::Value>,
        /// Tool definitions available for the model to call.
        #[serde(default)]
        tools: Vec<serde_json::Value>,
    },
    /// Non-streaming chat completion.
    ///
    /// The plugin responds with [`PluginIpcResponse::ChatCompletionResult`].
    ChatCompletion {
        /// Unique request identifier.
        request_id: String,
        /// Provider kind (e.g. `"anthropic"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Model identifier.
        model: String,
        /// Maximum tokens to generate.
        max_tokens: Option<u32>,
        /// Chat messages in provider-agnostic JSON format.
        messages: Vec<serde_json::Value>,
        /// Optional JSON Schema for structured output.
        json_schema: Option<serde_json::Value>,
    },
    /// Synthesize speech from text.
    ///
    /// The plugin responds with [`PluginIpcResponse::SpeechResult`].
    SynthesizeSpeech {
        /// Unique request identifier.
        request_id: String,
        /// Provider kind (e.g. `"voicevox"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Text to synthesize.
        text: String,
        /// Voice name.
        voice: String,
        /// Output audio format (e.g. `"wav"`).
        #[serde(default = "default_audio_format")]
        format: String,
    },
    /// Transcribe speech to text.
    ///
    /// The plugin responds with [`PluginIpcResponse::TranscriptionResult`].
    TranscribeAudio {
        /// Unique request identifier.
        request_id: String,
        /// Provider kind (e.g. `"whisper"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Base64-encoded audio data.
        audio_base64: String,
        /// Input audio format (e.g. `"wav"`).
        #[serde(default = "default_audio_format")]
        format: String,
    },
    /// Process one fixed-size PCM chunk through a voice activity detection
    /// engine.
    ///
    /// The plugin responds with [`PluginIpcResponse::VadChunkResult`].
    /// Engine state is per `session_id`: the host's engine adapter generates
    /// a unique id per `VadEngine` instance and keeps sending chunks for the
    /// lifetime of that instance. `reset` (sent with an empty `pcm`) clears
    /// the session's state, mirroring `VadEngine::reset`.
    ProcessVadChunk {
        /// Unique request identifier.
        request_id: String,
        /// Engine kind (e.g. `"silero"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Opaque session identifier correlating chunks to one engine state.
        session_id: String,
        /// PCM samples to process; empty when `reset` is set.
        pcm: Vec<f32>,
        /// When set, discard the session's state instead of processing.
        #[serde(default)]
        reset: bool,
    },
    /// Batch embedding request.
    ///
    /// The plugin responds with [`PluginIpcResponse::EmbedBatchResult`].
    EmbedBatch {
        /// Unique request identifier.
        request_id: String,
        /// Provider kind (e.g. `"openai"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Embedding model identifier.
        model: String,
        /// Desired embedding dimensions (provider may ignore).
        dimensions: Option<u32>,
        /// Text items to embed.
        items: Vec<String>,
    },
    /// Mediated call to a capability this plugin provides.
    ///
    /// The host routes a consumer's capability call here after resolving and
    /// authenticating it; the plugin responds with
    /// [`PluginIpcResponse::CapabilityCallResult`].
    CapabilityCall {
        /// Unique request identifier for correlating the response.
        request_id: String,
        /// The capability call to execute.
        call: CapabilityCall,
    },
}

/// Plugin IPC response — plugin → host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginIpcResponse {
    /// Handshake acknowledgment with negotiated version and capabilities.
    HandshakeAck {
        /// Agreed protocol version.
        version: u32,
        /// Capabilities advertised by this plugin.
        capabilities: PluginCapabilities,
    },
    /// Acknowledgment (for `ApprovePermission`, `AllowPattern`, etc.).
    Ack {
        /// Request identifier correlating this ack to the originating request.
        #[serde(default)]
        request_id: String,
    },
    /// Reply to a [`PluginIpcRequest::Ping`] liveness probe.
    ///
    /// Echoes the originating probe's `request_id` so the host's single reader
    /// task can correlate it. Older plugins that emit a bare `Pong`
    /// still interoperate because the field is `#[serde(default)]`.
    Pong {
        /// Request identifier correlating this pong to the originating `Ping`.
        #[serde(default)]
        request_id: String,
    },
    /// The plugin's config JSON Schema.
    ConfigSchema {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// The schema, or `None` if not provided.
        schema: Option<serde_json::Value>,
        /// Current config schema version (same meaning as
        /// [`PluginCapabilities::config_version`]). Older plugins omit the
        /// field; `#[serde(default)]` yields `0`.
        #[serde(default)]
        config_version: u32,
    },
    /// Acknowledgment that a [`PluginIpcRequest::SetConfig`] was applied.
    ConfigApplied {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
    },
    /// Dynamic options for a config path.
    ConfigOptions {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// Available options for the requested path.
        options: Vec<ConfigOption>,
    },
    /// Result of plugin-delegated config validation.
    ///
    /// An empty `errors` list means the value is valid.
    ConfigValidated {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// Field-level errors; empty means success.
        #[serde(default)]
        errors: Vec<ConfigFieldError>,
    },
    /// Result of a config migration.
    ConfigMigrated {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// Migrated configuration JSON.
        value: serde_json::Value,
        /// Version the migrated blob now corresponds to.
        #[serde(default)]
        config_version: u32,
    },
    /// Push notification: the plugin's config schema changed at runtime.
    ///
    /// Routed like [`DeferredCompleted`](Self::DeferredCompleted) — no
    /// `request_id`. The host caches the latest push so a UI poll can observe
    /// it without an in-flight waiter. Callers may also re-fetch via
    /// [`PluginIpcRequest::GetConfigSchema`].
    ConfigSchemaChanged {
        /// The updated schema, or `None` if the plugin cleared it.
        schema: Option<serde_json::Value>,
        /// Updated config schema version after the change.
        #[serde(default)]
        config_version: u32,
    },
    /// Error response.
    Error {
        /// Request identifier correlating this error to the originating request.
        #[serde(default)]
        request_id: String,
        /// Error description.
        message: String,
    },
    /// List of tool specs.
    Tools {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// The structured tool specs.
        tools: Vec<ToolSpec>,
    },
    /// Result of a tool call.
    CallResult {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// The structured result, or an error.
        result: Result<ToolResult, ToolError>,
    },
    /// Acknowledgment of a deferred (background) tool call.
    DeferredAccepted {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// Unique identifier for the queued background task.
        task_id: String,
    },
    /// Status of a deferred (background) task.
    DeferredStatus {
        /// Request identifier correlating this response to the originating request.
        #[serde(default)]
        request_id: String,
        /// The polled task id.
        task_id: String,
        /// Current status of the task.
        status: DeferredStatus,
    },
    /// Push notification: a deferred background task has completed.
    DeferredCompleted {
        /// The `task_id` from the original `DeferredAccepted`.
        task_id: String,
        /// Result of the deferred execution.
        result: Result<ToolResult, ToolError>,
    },
    /// A streaming chunk for an in-progress chat stream.
    ///
    /// Multiple chunks are sent per `request_id`, each carrying incremental
    /// text and/or tool-call deltas.
    StreamChunk {
        /// Request identifier correlating this chunk to the originating
        /// [`PluginIpcRequest::CreateChatStream`].
        request_id: String,
        /// Incremental text content (may be empty if only tool calls advance).
        #[serde(default)]
        text_delta: String,
        /// Incremental tool-call JSON (partial function-call arguments).
        #[serde(default)]
        tool_calls_delta: Vec<serde_json::Value>,
        /// Token usage, carried on the final chunk of the stream when the
        /// provider reports it. Intermediate chunks leave this `None`;
        /// older plugins omit the field entirely and deserialize to `None`.
        #[serde(default)]
        usage: Option<TokenUsage>,
    },
    /// Terminal message indicating a stream completed successfully.
    StreamEnd {
        /// Request identifier for the completed stream.
        request_id: String,
    },
    /// Terminal message indicating a stream failed.
    StreamError {
        /// Request identifier for the failed stream.
        request_id: String,
        /// Human-readable error description.
        message: String,
    },
    /// Result of a non-streaming chat completion.
    ChatCompletionResult {
        /// Request identifier.
        request_id: String,
        /// The generated content.
        content: String,
        /// Token usage reported by the provider, if any. `None` when
        /// the provider does not report usage (the host then falls back to a
        /// character-based estimate); older plugins omit the field and
        /// deserialize to `None`.
        #[serde(default)]
        usage: Option<TokenUsage>,
    },
    /// Result of speech synthesis.
    SpeechResult {
        /// Request identifier correlating this response to the originating
        /// [`PluginIpcRequest::SynthesizeSpeech`].
        request_id: String,
        /// Base64-encoded audio data.
        audio_base64: String,
        /// Audio format (matches request format).
        format: String,
    },
    /// Result of speech transcription.
    TranscriptionResult {
        /// Request identifier correlating this response to the originating
        /// [`PluginIpcRequest::TranscribeAudio`].
        request_id: String,
        /// Transcribed text.
        text: String,
        /// Detected language code (e.g. `"ja"`, `"en"`), when the plugin
        /// knows it. Older plugins omit the field and deserialize to `None`.
        #[serde(default)]
        language: Option<String>,
    },
    /// Result of one [`PluginIpcRequest::ProcessVadChunk`] step.
    VadChunkResult {
        /// Request identifier correlating this response to the originating
        /// [`PluginIpcRequest::ProcessVadChunk`].
        request_id: String,
        /// Voice activity event for the processed chunk.
        event: VadEvent,
    },
    /// Result of a batch embedding request.
    EmbedBatchResult {
        /// Request identifier.
        request_id: String,
        /// Embedding vectors, one per input item.
        embeddings: Vec<Vec<f32>>,
    },
    /// Result of a mediated [`PluginIpcRequest::CapabilityCall`].
    CapabilityCallResult {
        /// Unique request identifier correlating to the originating request.
        request_id: String,
        /// The provider's JSON result or a typed capability error.
        result: CapabilityCallResult,
    },
}

/// Voice activity event emitted by a VAD engine per processed chunk.
///
/// Mirrors the host-side `ene_ai::VadEvent` states one-to-one; the host
/// adapter maps between the two so the wire stays a plain contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VadEvent {
    /// Speech just started.
    SpeechStart,
    /// Speech is continuing.
    SpeechContinue,
    /// Speech just ended.
    SpeechEnd,
    /// No speech detected.
    Silence,
}

/// Reads a [`PluginIpcRequest`] as a 4-byte LE length-prefixed payload in
/// `format`.
///
/// Returns `Ok(None)` on `UnexpectedEof`, indicating connection closed.
pub async fn read_plugin_request<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    format: WireFormat,
) -> Result<Option<PluginIpcRequest>, PluginError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    if len > MAX_MESSAGE_SIZE {
        return Err(PluginError::protocol(format!(
            "Request size {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    let req = format.decode(&buf).map_err(|e| {
        PluginError::protocol(format!("Failed to deserialize PluginIpcRequest: {e}"))
    })?;
    Ok(Some(req))
}

/// Writes a [`PluginIpcRequest`] as a 4-byte LE length-prefixed payload in
/// `format`.
///
/// # Errors
///
/// Returns [`PluginError::Protocol`] if the serialized message exceeds
/// [`MAX_MESSAGE_SIZE`], preventing oversized messages from being silently
/// sent to the peer.
pub async fn write_plugin_request<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    req: &PluginIpcRequest,
    format: WireFormat,
) -> Result<(), PluginError> {
    let payload = format
        .encode(req)
        .map_err(|e| PluginError::protocol(format!("Failed to serialize PluginIpcRequest: {e}")))?;
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(PluginError::protocol(format!(
            "Request size {} exceeds maximum {MAX_MESSAGE_SIZE}",
            payload.len()
        )));
    }
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads a [`PluginIpcResponse`] as a 4-byte LE length-prefixed payload in
/// `format`.
///
/// Returns `Ok(None)` on `UnexpectedEof`, indicating connection closed.
///
/// # Correlation
///
/// All non-streaming responses carry a `request_id` field that correlates
/// the response to the originating [`PluginIpcRequest`]. Callers should
/// verify that the returned `request_id` matches the expected value.
/// Streaming responses (`StreamChunk`, `StreamEnd`, `StreamError`) also
/// carry a `request_id` for correlation.
pub async fn read_plugin_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    format: WireFormat,
) -> Result<Option<PluginIpcResponse>, PluginError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    if len > MAX_MESSAGE_SIZE {
        return Err(PluginError::protocol(format!(
            "Response size {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    let resp = format.decode(&buf).map_err(|e| {
        PluginError::protocol(format!("Failed to deserialize PluginIpcResponse: {e}"))
    })?;
    Ok(Some(resp))
}

/// Writes a [`PluginIpcResponse`] as a 4-byte LE length-prefixed payload in
/// `format`.
///
/// # Errors
///
/// Returns [`PluginError::Protocol`] if the serialized message exceeds
/// [`MAX_MESSAGE_SIZE`], preventing oversized messages from being silently
/// sent to the peer.
pub async fn write_plugin_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    resp: &PluginIpcResponse,
    format: WireFormat,
) -> Result<(), PluginError> {
    let payload = format.encode(resp).map_err(|e| {
        PluginError::protocol(format!("Failed to serialize PluginIpcResponse: {e}"))
    })?;
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(PluginError::protocol(format!(
            "Response size {} exceeds maximum {MAX_MESSAGE_SIZE}",
            payload.len()
        )));
    }
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::LlmProviderSpec;

    async fn send_recv_request(req: &PluginIpcRequest) -> PluginIpcRequest {
        send_recv_request_with(req, WireFormat::Json).await;
        send_recv_request_with(req, WireFormat::MsgPack).await
    }

    async fn send_recv_response(resp: &PluginIpcResponse) -> PluginIpcResponse {
        send_recv_response_with(resp, WireFormat::Json).await;
        send_recv_response_with(resp, WireFormat::MsgPack).await
    }

    async fn send_recv_request_with(
        req: &PluginIpcRequest,
        format: WireFormat,
    ) -> PluginIpcRequest {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        write_plugin_request(&mut a, req, format).await.unwrap();
        drop(a);
        let got = read_plugin_request(&mut b, format).await.unwrap().unwrap();
        assert_eq!(&got, req);
        got
    }

    async fn send_recv_response_with(
        resp: &PluginIpcResponse,
        format: WireFormat,
    ) -> PluginIpcResponse {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        write_plugin_response(&mut a, resp, format).await.unwrap();
        drop(a);
        let got = read_plugin_response(&mut b, format).await.unwrap().unwrap();
        assert_eq!(&got, resp);
        got
    }

    #[tokio::test]
    async fn request_handshake_roundtrip() {
        let req = PluginIpcRequest::Handshake {
            version: VersionRange::host_supported(),
            sandbox: SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"api_key": "sk-test"})),
            plugin_profiles: Some(serde_json::json!({"default": {"voice": "af_heart"}})),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_handshake_profiles_default_keeps_old_peers_compatible() {
        // A plugin built against protocol v3 sends no `plugin_profiles`; the
        // field must default to `None` rather than failing deserialization.
        let json = r#"{
            "Handshake": {
                "version": {"min": 3, "max": 4},
                "sandbox": {},
                "plugin_config": {"api_key": "sk-test"}
            }
        }"#;
        let got: PluginIpcRequest =
            serde_json::from_str(json).expect("deserialize without profiles");
        assert!(matches!(
            got,
            PluginIpcRequest::Handshake {
                plugin_profiles: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn request_shutdown_roundtrip() {
        let req = PluginIpcRequest::Shutdown;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_ping_roundtrip() {
        let req = PluginIpcRequest::Ping {
            request_id: "ping-1".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_get_config_schema_roundtrip() {
        let req = PluginIpcRequest::GetConfigSchema {
            request_id: "req-1".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_list_tools_roundtrip() {
        let req = PluginIpcRequest::ListTools {
            request_id: "req-1".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_call_tool_roundtrip() {
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "filesystem.read".into(),
            arguments: r#"{"path":"/tmp/test.txt"}"#.into(),
            deferred: false,
            context: None,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_call_tool_deferred_roundtrip() {
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "timer.set".into(),
            arguments: r#"{"seconds":5}"#.into(),
            deferred: true,
            context: None,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_set_call_context_roundtrip() {
        let req = PluginIpcRequest::SetCallContext {
            request_id: "req-1".into(),
            conversation_id: "conv-1".into(),
            turn_id: "turn-42".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_poll_deferred_roundtrip() {
        let req = PluginIpcRequest::PollDeferred {
            request_id: "req-1".into(),
            task_id: "task-123".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_cancel_deferred_roundtrip() {
        let req = PluginIpcRequest::CancelDeferred {
            request_id: "req-1".into(),
            task_id: "task-456".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_approve_permission_roundtrip() {
        let req = PluginIpcRequest::ApprovePermission {
            request_id: "req-1".into(),
            permission_request_id: "perm-1".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_allow_pattern_roundtrip() {
        let req = PluginIpcRequest::AllowPattern {
            request_id: "req-1".into(),
            action: "filesystem_write".into(),
            target_pattern: "/tmp/**".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_revoke_pattern_roundtrip() {
        let req = PluginIpcRequest::RevokePattern {
            request_id: "req-1".into(),
            action: "filesystem_write".into(),
            target_pattern: "/tmp/**".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_create_chat_stream_roundtrip() {
        let req = PluginIpcRequest::CreateChatStream {
            request_id: "req-uuid-1".into(),
            provider_kind: "anthropic".into(),
            provider_config: serde_json::json!({"api_key": "sk-ant-test"}),
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: Some(4096),
            messages: vec![serde_json::json!({"role": "user", "content": "Hello"})],
            tools: vec![serde_json::json!({"name": "read", "description": "Read a file"})],
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_chat_completion_roundtrip() {
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-uuid-2".into(),
            provider_kind: "anthropic".into(),
            provider_config: serde_json::json!({}),
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: None,
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            json_schema: Some(serde_json::json!({"type": "object"})),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_embed_batch_roundtrip() {
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-uuid-3".into(),
            provider_kind: "openai_compatible".into(),
            provider_config: serde_json::json!({"base_url": "http://localhost:11434"}),
            model: "text-embedding-3-small".into(),
            dimensions: Some(1536),
            items: vec!["hello world".into(), "foo bar".into()],
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn response_handshake_ack_roundtrip() {
        let resp = PluginIpcResponse::HandshakeAck {
            version: PLUGIN_IPC_PROTOCOL_VERSION,
            capabilities: PluginCapabilities {
                tools: 0,
                llm_providers: vec![LlmProviderSpec {
                    kind: "anthropic".into(),
                    supported_models: vec!["claude-sonnet-4-20250514".into()],
                    supports_streaming: true,
                    supports_vision: true,
                    concurrency: crate::capabilities::ConcurrencyHint::default(),
                    context_window: None,
                    resource_class: crate::capabilities::ResourceClass::default(),
                }],
                tts_providers: vec![],
                stt_providers: vec![],
                ..PluginCapabilities::default()
            },
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_ack_roundtrip() {
        let resp = PluginIpcResponse::Ack {
            request_id: "req-1".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_pong_roundtrip() {
        let resp = PluginIpcResponse::Pong {
            request_id: "ping-1".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_config_schema_roundtrip() {
        let resp = PluginIpcResponse::ConfigSchema {
            request_id: "req-1".into(),
            schema: Some(serde_json::json!({"type": "object", "properties": {}})),
            config_version: 1,
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_config_schema_omitted_version_defaults_to_zero() {
        let json = r#"{"ConfigSchema":{"request_id":"r1","schema":null}}"#;
        let resp: PluginIpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp,
            PluginIpcResponse::ConfigSchema {
                request_id: "r1".into(),
                schema: None,
                config_version: 0,
            }
        );
    }

    #[tokio::test]
    async fn request_list_config_options_roundtrip() {
        let req = PluginIpcRequest::ListConfigOptions {
            request_id: "req-opt".into(),
            path: "voice".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn response_config_options_roundtrip() {
        let resp = PluginIpcResponse::ConfigOptions {
            request_id: "req-opt".into(),
            options: vec![ConfigOption {
                value: serde_json::json!("alloy"),
                label: "Alloy".into(),
                group: Some("openai".into()),
            }],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn request_validate_config_roundtrip() {
        let req = PluginIpcRequest::ValidateConfig {
            request_id: "req-val".into(),
            value: serde_json::json!({"voice": "alloy"}),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn response_config_validated_roundtrip() {
        let resp = PluginIpcResponse::ConfigValidated {
            request_id: "req-val".into(),
            errors: vec![ConfigFieldError {
                field_path: "mmproj_path".into(),
                message: "required when multimodal model is selected".into(),
            }],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn request_migrate_config_roundtrip() {
        let req = PluginIpcRequest::MigrateConfig {
            request_id: "req-mig".into(),
            from_version: 1,
            value: serde_json::json!({"speaker": 1}),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn response_config_migrated_roundtrip() {
        let resp = PluginIpcResponse::ConfigMigrated {
            request_id: "req-mig".into(),
            value: serde_json::json!({"speaker": {"id": 1}}),
            config_version: 2,
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_config_schema_changed_roundtrip() {
        let resp = PluginIpcResponse::ConfigSchemaChanged {
            schema: Some(serde_json::json!({"type": "object"})),
            config_version: 3,
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_error_roundtrip() {
        let resp = PluginIpcResponse::Error {
            request_id: "req-1".into(),
            message: "something went wrong".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_tools_roundtrip() {
        let resp = PluginIpcResponse::Tools {
            request_id: "req-1".into(),
            tools: vec![ToolSpec::new(
                crate::ToolName::new("test"),
                "desc",
                serde_json::json!({}),
            )],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_call_result_ok_roundtrip() {
        let resp = PluginIpcResponse::CallResult {
            request_id: "req-1".into(),
            result: Ok(ToolResult::text("success")),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_call_result_err_roundtrip() {
        let resp = PluginIpcResponse::CallResult {
            request_id: "req-1".into(),
            result: Err(ToolError::NotFound {
                tool_name: "foo".into(),
            }),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_deferred_accepted_roundtrip() {
        let resp = PluginIpcResponse::DeferredAccepted {
            request_id: "req-1".into(),
            task_id: "task-789".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_deferred_status_roundtrip() {
        let resp = PluginIpcResponse::DeferredStatus {
            request_id: "req-1".into(),
            task_id: "task-abc".into(),
            status: DeferredStatus::Completed {
                result: ToolResult::text("done"),
            },
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_deferred_completed_roundtrip() {
        // `DeferredCompleted` is a push message: it carries a `task_id` and a
        // structured `ToolResult` but no `request_id`.
        let resp = PluginIpcResponse::DeferredCompleted {
            task_id: "task-xyz".into(),
            result: Ok(ToolResult::text("background done")),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_deferred_completed_error_roundtrip() {
        let resp = PluginIpcResponse::DeferredCompleted {
            task_id: "task-err".into(),
            result: Err(ToolError::ipc_client("boom")),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_chunk_roundtrip() {
        let resp = PluginIpcResponse::StreamChunk {
            request_id: "req-uuid-1".into(),
            text_delta: "Hello, ".into(),
            tool_calls_delta: vec![],
            usage: None,
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_chunk_with_tool_calls_roundtrip() {
        let resp = PluginIpcResponse::StreamChunk {
            request_id: "req-uuid-1".into(),
            text_delta: String::new(),
            tool_calls_delta: vec![
                serde_json::json!({"id": "tc_1", "name": "read", "arguments": "{\"pa"}),
            ],
            usage: None,
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_end_roundtrip() {
        let resp = PluginIpcResponse::StreamEnd {
            request_id: "req-uuid-1".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_error_roundtrip() {
        let resp = PluginIpcResponse::StreamError {
            request_id: "req-uuid-1".into(),
            message: "rate limit exceeded".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_chat_completion_result_roundtrip() {
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "req-uuid-2".into(),
            content: "The answer is 42.".into(),
            usage: None,
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_chat_completion_result_with_usage_roundtrip() {
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "req-uuid-2".into(),
            content: "The answer is 42.".into(),
            usage: Some(TokenUsage::new(12, 8, 20)),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_chunk_with_usage_roundtrip() {
        let resp = PluginIpcResponse::StreamChunk {
            request_id: "req-uuid-1".into(),
            text_delta: String::new(),
            tool_calls_delta: vec![],
            usage: Some(TokenUsage::new(100, 50, 150)),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_embed_batch_result_roundtrip() {
        let resp = PluginIpcResponse::EmbedBatchResult {
            request_id: "req-uuid-3".into(),
            embeddings: vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn request_synthesize_speech_roundtrip() {
        let req = PluginIpcRequest::SynthesizeSpeech {
            request_id: "req-tts-1".into(),
            provider_kind: "voicevox".into(),
            provider_config: serde_json::json!({"speaker": 1}),
            text: "Hello world".into(),
            voice: "default".into(),
            format: "wav".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_transcribe_audio_roundtrip() {
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "whisper".into(),
            provider_config: serde_json::json!({"model": "whisper-1"}),
            audio_base64: "AAAA".into(),
            format: "wav".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_process_vad_chunk_roundtrip() {
        let req = PluginIpcRequest::ProcessVadChunk {
            request_id: "req-vad-1".into(),
            provider_kind: "silero".into(),
            provider_config: serde_json::json!({"threshold": 0.5}),
            session_id: "session-1".into(),
            pcm: vec![0.0; 512],
            reset: false,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_process_vad_chunk_omitted_reset_defaults_to_false() {
        let json = r#"{"ProcessVadChunk":{"request_id":"r1","provider_kind":"silero","provider_config":{},"session_id":"s1","pcm":[0.0]}}"#;
        let req: PluginIpcRequest = serde_json::from_str(json).unwrap();
        let PluginIpcRequest::ProcessVadChunk { reset, .. } = req else {
            panic!("expected ProcessVadChunk");
        };
        assert!(!reset);
    }

    #[tokio::test]
    async fn request_set_config_roundtrip() {
        let req = PluginIpcRequest::SetConfig {
            request_id: "req-cfg-1".into(),
            config: serde_json::json!({"api_key": {"source": "env"}}),
            profiles: Some(serde_json::json!({"default": {"voice": "af_heart"}})),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_set_config_omitted_profiles_defaults_to_none() {
        let json = r#"{"SetConfig":{"request_id":"r1","config":{"k":1}}}"#;
        let req: PluginIpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req,
            PluginIpcRequest::SetConfig {
                request_id: "r1".into(),
                config: serde_json::json!({"k": 1}),
                profiles: None,
            }
        );
    }

    #[tokio::test]
    async fn response_config_applied_roundtrip() {
        let resp = PluginIpcResponse::ConfigApplied {
            request_id: "req-cfg-1".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_speech_result_roundtrip() {
        let resp = PluginIpcResponse::SpeechResult {
            request_id: "req-tts-1".into(),
            audio_base64: "AAAA".into(),
            format: "wav".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_transcription_result_roundtrip() {
        let resp = PluginIpcResponse::TranscriptionResult {
            request_id: "req-stt-1".into(),
            text: "Hello world".into(),
            language: Some("en".into()),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[test]
    fn transcription_result_missing_language_defaults_to_none() {
        let json = r#"{"TranscriptionResult":{"request_id":"r1","text":"hello"}}"#;
        let resp: PluginIpcResponse = serde_json::from_str(json).unwrap();
        assert!(matches!(
            resp,
            PluginIpcResponse::TranscriptionResult { language: None, .. }
        ));
    }

    #[tokio::test]
    async fn response_vad_chunk_result_roundtrip() {
        for event in [
            VadEvent::SpeechStart,
            VadEvent::SpeechContinue,
            VadEvent::SpeechEnd,
            VadEvent::Silence,
        ] {
            let resp = PluginIpcResponse::VadChunkResult {
                request_id: "req-vad-1".into(),
                event,
            };
            let got = send_recv_response(&resp).await;
            assert_eq!(got, resp);
        }
    }

    #[test]
    fn vad_event_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&VadEvent::SpeechStart).unwrap(),
            r#""speech_start""#
        );
        assert_eq!(
            serde_json::from_str::<VadEvent>(r#""speech_end""#).unwrap(),
            VadEvent::SpeechEnd
        );
    }

    #[tokio::test]
    async fn read_request_eof_returns_none() {
        let mut buf: &[u8] = &[];
        let result = read_plugin_request(&mut buf, WireFormat::Json)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn version_range_negotiate_exact_overlap() {
        let a = VersionRange { min: 4, max: 4 };
        let b = VersionRange { min: 4, max: 4 };
        assert_eq!(a.negotiate(&b), Some(4));
    }

    #[test]
    fn version_range_negotiate_partial_overlap_picks_highest_common() {
        // host {3,4} ∩ plugin {4,4} → highest common version is 4.
        let host = VersionRange { min: 3, max: 4 };
        let plugin = VersionRange { min: 4, max: 4 };
        assert_eq!(host.negotiate(&plugin), Some(4));
        assert_eq!(plugin.negotiate(&host), Some(4));
    }

    #[test]
    fn version_range_negotiate_wide_overlap_picks_highest_common() {
        // {2,5} ∩ {3,4} → highest common is min(5,4) = 4.
        let a = VersionRange { min: 2, max: 5 };
        let b = VersionRange { min: 3, max: 4 };
        assert_eq!(a.negotiate(&b), Some(4));
    }

    #[test]
    fn version_range_negotiate_no_overlap_is_none() {
        let a = VersionRange { min: 1, max: 2 };
        let b = VersionRange { min: 4, max: 4 };
        assert_eq!(a.negotiate(&b), None);
        assert_eq!(b.negotiate(&a), None);
    }

    #[test]
    fn version_range_contains() {
        let r = VersionRange { min: 3, max: 4 };
        assert!(!r.contains(2));
        assert!(r.contains(3));
        assert!(r.contains(4));
        assert!(!r.contains(5));
    }

    #[test]
    fn min_supported_version_is_exactly_one_behind_current() {
        // N-1 support policy: the host maintains exactly one version of
        // backward compatibility.
        assert_eq!(
            PLUGIN_IPC_MIN_SUPPORTED_VERSION,
            PLUGIN_IPC_PROTOCOL_VERSION - 1
        );
    }

    #[test]
    fn host_supported_spans_min_to_current() {
        let range = VersionRange::host_supported();
        assert_eq!(range.min, PLUGIN_IPC_MIN_SUPPORTED_VERSION);
        assert_eq!(range.max, PLUGIN_IPC_PROTOCOL_VERSION);
        // A plugin built against the previous protocol version must still
        // be able to negotiate a connection.
        let old_plugin = VersionRange {
            min: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
            max: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
        };
        assert_eq!(
            range.negotiate(&old_plugin),
            Some(PLUGIN_IPC_MIN_SUPPORTED_VERSION)
        );
        // A plugin two versions behind is outside the supported window.
        let ancient_plugin = VersionRange {
            min: PLUGIN_IPC_MIN_SUPPORTED_VERSION - 1,
            max: PLUGIN_IPC_MIN_SUPPORTED_VERSION - 1,
        };
        assert_eq!(range.negotiate(&ancient_plugin), None);
    }

    #[tokio::test]
    async fn read_response_eof_returns_none() {
        let mut buf: &[u8] = &[];
        let result = read_plugin_response(&mut buf, WireFormat::Json)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn zero_length_request_returns_none() {
        let mut buf: &[u8] = &[0u8; 4];
        let result = read_plugin_request(&mut buf, WireFormat::Json)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn streaming_sequence_roundtrip() {
        let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);

        let chunks = vec![
            PluginIpcResponse::StreamChunk {
                request_id: "stream-1".into(),
                text_delta: "Hello".into(),
                tool_calls_delta: vec![],
                usage: None,
            },
            PluginIpcResponse::StreamChunk {
                request_id: "stream-1".into(),
                text_delta: ", world!".into(),
                tool_calls_delta: vec![],
                usage: Some(TokenUsage::new(5, 2, 7)),
            },
            PluginIpcResponse::StreamEnd {
                request_id: "stream-1".into(),
            },
        ];

        let expected = chunks.clone();
        let write_handle = tokio::spawn(async move {
            for chunk in &chunks {
                write_plugin_response(&mut writer, chunk, WireFormat::MsgPack)
                    .await
                    .unwrap();
            }
        });

        let mut received = Vec::new();
        loop {
            let resp = read_plugin_response(&mut reader, WireFormat::MsgPack)
                .await
                .unwrap()
                .unwrap();
            let is_terminal = matches!(
                resp,
                PluginIpcResponse::StreamEnd { .. } | PluginIpcResponse::StreamError { .. }
            );
            received.push(resp);
            if is_terminal {
                break;
            }
        }

        write_handle.await.unwrap();
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn large_payload_roundtrip() {
        let big_content = "x".repeat(100_000);
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "big-1".into(),
            content: big_content.clone(),
            usage: None,
        };
        let (mut a, mut b) = tokio::io::duplex(256 * 1024);
        write_plugin_response(&mut a, &resp, WireFormat::MsgPack)
            .await
            .unwrap();
        drop(a);
        let got = read_plugin_response(&mut b, WireFormat::MsgPack)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn call_tool_defaults_to_sync_and_no_request_id() {
        let json = r#"{"CallTool":{"name":"read","arguments":"{}"}}"#;
        let req: PluginIpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req,
            PluginIpcRequest::CallTool {
                request_id: String::new(),
                name: "read".into(),
                arguments: "{}".into(),
                deferred: false,
                context: None,
            }
        );
    }

    #[tokio::test]
    async fn read_request_rejects_oversized_message() {
        let oversized_len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut buf = oversized_len.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        let mut cursor: &[u8] = &buf;
        let result = read_plugin_request(&mut cursor, WireFormat::Json).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn read_response_rejects_oversized_message() {
        let oversized_len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut buf = oversized_len.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        let mut cursor: &[u8] = &buf;
        let result = read_plugin_response(&mut cursor, WireFormat::Json).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn write_request_rejects_oversized_message() {
        let big_arguments = "x".repeat(MAX_MESSAGE_SIZE + 1);
        let req = PluginIpcRequest::CallTool {
            request_id: String::new(),
            name: "test".into(),
            arguments: big_arguments,
            deferred: false,
            context: None,
        };
        let mut buf = Vec::new();
        let result = write_plugin_request(&mut buf, &req, WireFormat::Json).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(buf.is_empty(), "nothing must be written on rejection");
    }

    #[tokio::test]
    async fn write_response_rejects_oversized_message() {
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "big-1".into(),
            content: "x".repeat(MAX_MESSAGE_SIZE + 1),
            usage: None,
        };
        let mut buf = Vec::new();
        let result = write_plugin_response(&mut buf, &resp, WireFormat::Json).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(buf.is_empty(), "nothing must be written on rejection");
    }

    #[test]
    fn wire_format_maps_protocol_versions() {
        assert_eq!(WireFormat::for_version(0), WireFormat::Json);
        assert_eq!(
            WireFormat::for_version(WireFormat::MSGPACK_MIN_PROTOCOL_VERSION - 1),
            WireFormat::Json
        );
        assert_eq!(
            WireFormat::for_version(WireFormat::MSGPACK_MIN_PROTOCOL_VERSION),
            WireFormat::MsgPack
        );
        assert_eq!(
            WireFormat::for_version(PLUGIN_IPC_PROTOCOL_VERSION),
            WireFormat::MsgPack
        );
        assert_eq!(
            WireFormat::for_version(PLUGIN_IPC_PROTOCOL_VERSION + 1),
            WireFormat::MsgPack
        );
    }

    #[tokio::test]
    async fn handshake_json_then_msgpack_frames_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);

        // The handshake exchange always uses JSON framing; only frames after
        // the ack switch to the negotiated format (v6 here).
        let handshake = PluginIpcRequest::Handshake {
            version: VersionRange::host_supported(),
            sandbox: SandboxConfigData::default(),
            plugin_config: None,
            plugin_profiles: None,
        };
        write_plugin_request(&mut a, &handshake, WireFormat::Json)
            .await
            .unwrap();
        let got = read_plugin_request(&mut b, WireFormat::Json)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, handshake);

        let ack = PluginIpcResponse::HandshakeAck {
            version: PLUGIN_IPC_PROTOCOL_VERSION,
            capabilities: PluginCapabilities::default(),
        };
        write_plugin_response(&mut a, &ack, WireFormat::Json)
            .await
            .unwrap();
        let got = read_plugin_response(&mut b, WireFormat::Json)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, ack);

        let list = PluginIpcRequest::ListTools {
            request_id: "r1".into(),
        };
        write_plugin_request(&mut a, &list, WireFormat::MsgPack)
            .await
            .unwrap();
        let got = read_plugin_request(&mut b, WireFormat::MsgPack)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, list);
    }

    #[test]
    fn msgpack_embeddings_smaller_than_json() {
        let embeddings = (0..64)
            .map(|i| {
                (0..256)
                    .map(|j| ((i * 256 + j) as f32 * 0.37).sin())
                    .collect::<Vec<f32>>()
            })
            .collect::<Vec<Vec<f32>>>();
        let resp = PluginIpcResponse::EmbedBatchResult {
            request_id: "emb-1".into(),
            embeddings,
        };
        let json_len = WireFormat::Json.encode(&resp).unwrap().len();
        let msgpack_len = WireFormat::MsgPack.encode(&resp).unwrap().len();
        assert!(
            msgpack_len < json_len,
            "msgpack {msgpack_len} bytes >= json {json_len} bytes"
        );
    }

    #[test]
    fn msgpack_embeddings_preserve_non_finite_floats() {
        let resp = PluginIpcResponse::EmbedBatchResult {
            request_id: "emb-nan".into(),
            embeddings: vec![vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5]],
        };
        // JSON encodes non-finite floats as `null`, which cannot decode back
        // into an `f32`; MessagePack round-trips them natively.
        let json = serde_json::to_vec(&resp).expect("JSON encodes non-finite floats as null");
        assert!(
            serde_json::from_slice::<PluginIpcResponse>(&json).is_err(),
            "JSON must not round-trip non-finite floats"
        );
        let payload = WireFormat::MsgPack.encode(&resp).unwrap();
        let got: PluginIpcResponse = WireFormat::MsgPack.decode(&payload).unwrap();
        let PluginIpcResponse::EmbedBatchResult { embeddings, .. } = &got else {
            panic!("expected EmbedBatchResult, got {got:?}");
        };
        assert!(embeddings[0][0].is_nan());
        assert!(embeddings[0][1].is_infinite() && embeddings[0][1].is_sign_positive());
        assert!(embeddings[0][2].is_infinite() && embeddings[0][2].is_sign_negative());
        assert_eq!(embeddings[0][3].to_bits(), 0.5f32.to_bits());
    }

    #[test]
    fn msgpack_tools_smaller_than_json() {
        let tools = (0..8)
            .map(|i| {
                ToolSpec::new(
                    crate::ToolName::new(format!("mock.tool{i}")),
                    "Echoes arguments back with a reasonably long description string.",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "text": {"type": "string", "description": "Input text to echo."},
                            "count": {"type": "integer", "minimum": 1, "maximum": 100},
                            "enabled": {"type": "boolean", "default": true},
                        },
                        "required": ["text"],
                    }),
                )
            })
            .collect::<Vec<_>>();
        let resp = PluginIpcResponse::Tools {
            request_id: "tools-1".into(),
            tools,
        };
        let json_len = WireFormat::Json.encode(&resp).unwrap().len();
        let msgpack_len = WireFormat::MsgPack.encode(&resp).unwrap().len();
        assert!(
            msgpack_len < json_len,
            "msgpack {msgpack_len} bytes >= json {json_len} bytes"
        );
    }

    #[tokio::test]
    async fn write_request_rejects_oversized_msgpack_message() {
        let big_arguments = "x".repeat(MAX_MESSAGE_SIZE + 1);
        let req = PluginIpcRequest::CallTool {
            request_id: String::new(),
            name: "test".into(),
            arguments: big_arguments,
            deferred: false,
            context: None,
        };
        let mut buf = Vec::new();
        let result = write_plugin_request(&mut buf, &req, WireFormat::MsgPack).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(buf.is_empty(), "nothing must be written on rejection");
    }

    #[tokio::test]
    async fn write_response_rejects_oversized_msgpack_message() {
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "big-1".into(),
            content: "x".repeat(MAX_MESSAGE_SIZE + 1),
            usage: None,
        };
        let mut buf = Vec::new();
        let result = write_plugin_response(&mut buf, &resp, WireFormat::MsgPack).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(buf.is_empty(), "nothing must be written on rejection");
    }
}
