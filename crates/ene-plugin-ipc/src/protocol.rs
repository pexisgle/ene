use serde::{Deserialize, Serialize};

use crate::provider::{
    EmbedRequest, EmbedResult, ListModelsRequest, ListModelsResult, LlmChunk, LlmGenerateRequest,
    LlmGeneration, ProviderFaces, SttRequest, SttResult, TtsAudio, TtsRequest,
};

/// Current `core` subprotocol version.
pub const CORE_VERSION: u32 = 1;
/// Current `tool` subprotocol version.
pub const TOOL_VERSION: u32 = 1;

/// Inclusive version window for one subprotocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    pub min: u32,
    pub max: u32,
}

impl VersionRange {
    #[must_use]
    pub const fn exact(version: u32) -> Self {
        Self {
            min: version,
            max: version,
        }
    }

    #[must_use]
    pub fn negotiate(self, other: Self) -> Option<u32> {
        let high = self.max.min(other.max);
        let low = self.min.max(other.min);
        (high >= low).then_some(high)
    }
}

/// Subprotocol identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtoId {
    Core,
    Tool,
    Provider,
    Capability,
}

impl ProtoId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Tool => "tool",
            Self::Provider => "provider",
            Self::Capability => "capability",
        }
    }
}

/// Host-advertised ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolRanges {
    pub core: VersionRange,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool: Option<VersionRange>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider: Option<VersionRange>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capability: Option<VersionRange>,
}

impl ProtocolRanges {
    #[must_use]
    pub fn host_supported() -> Self {
        Self {
            core: VersionRange::exact(CORE_VERSION),
            tool: Some(VersionRange::exact(TOOL_VERSION)),
            provider: Some(VersionRange::exact(crate::provider::PROVIDER_VERSION)),
            capability: None,
        }
    }
}

/// Negotiated versions. Missing optional protos are disabled, not fatal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    pub core: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider: Option<ProviderFaces>,
}

/// Host hello (first frame).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHello {
    pub host_name: String,
    pub host_version: String,
    pub protocols: ProtocolRanges,
    pub expected_digest: String,
    pub declared_protocols: Vec<ProtoId>,
    /// `0` means the compile-time default (`MAX_FRAME_BYTES`).
    #[serde(default)]
    pub max_frame_bytes: u32,
    /// Skip digest mismatch. Runtime source of truth is `plugins.policy.allow_unverified`.
    #[serde(default)]
    pub allow_unverified: bool,
}

impl HostHello {
    #[must_use]
    pub fn frame_limit(&self) -> usize {
        crate::frame::frame_limit(self.max_frame_bytes)
    }
}

/// Plugin `hello_ack`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    pub plugin_id: String,
    pub plugin_name: String,
    pub plugin_version: String,
    pub manifest_digest: String,
    pub protocols: Negotiated,
    #[serde(default)]
    pub spawn_token: String,
}

/// Fatal handshake rejection (core overlap failed or digest mismatch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReject {
    pub reason: String,
}

/// Wire tool spec. `side_effects` has no default — omit is a decode error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpecWire {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub output: serde_json::Value,
    pub side_effects: Vec<String>,
}

/// Tool call request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    #[serde(default)]
    pub deadline_ms: Option<u64>,
}

/// Tool result body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub status: String,
    pub value: serde_json::Value,
}

/// Envelope. `id` is required on request/response variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    Hello { body: HostHello },
    HelloAck { body: HelloAck },
    HelloReject { body: HelloReject },
    Ping { id: u64 },
    Pong { id: u64 },
    ToolList { id: u64 },
    ToolSpec { id: u64, tools: Vec<ToolSpecWire> },
    ToolCall { id: u64, body: ToolCall },
    ToolResult { id: u64, body: ToolResult },
    ToolCancel { id: u64, call_id: String },
    Shutdown { id: u64 },
    Drain { id: u64 },
    DrainAck { id: u64 },
    Log { fields: serde_json::Value },
    LlmGenerate { id: u64, body: LlmGenerateRequest },
    LlmChunk { id: u64, body: LlmChunk },
    LlmDone { id: u64, body: LlmGeneration },
    EmbedEncode { id: u64, body: EmbedRequest },
    EmbedResult { id: u64, body: EmbedResult },
    TtsSynthesize { id: u64, body: TtsRequest },
    TtsResult { id: u64, body: TtsAudio },
    SttTranscribe { id: u64, body: SttRequest },
    SttResult { id: u64, body: SttResult },
    ProviderListModels { id: u64, body: ListModelsRequest },
    ProviderModels { id: u64, body: ListModelsResult },
}

impl Message {
    pub fn encode(&self) -> Result<Vec<u8>, crate::IpcError> {
        rmp_serde::to_vec_named(self).map_err(crate::IpcError::codec)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, crate::IpcError> {
        rmp_serde::from_slice(bytes).map_err(crate::IpcError::codec)
    }

    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::HelloAck { .. } => "hello_ack",
            Self::HelloReject { .. } => "hello_reject",
            Self::Ping { .. } => "ping",
            Self::Pong { .. } => "pong",
            Self::ToolList { .. } => "tool_list",
            Self::ToolSpec { .. } => "tool_spec",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::ToolCancel { .. } => "tool_cancel",
            Self::Shutdown { .. } => "shutdown",
            Self::Drain { .. } => "drain",
            Self::DrainAck { .. } => "drain_ack",
            Self::Log { .. } => "log",
            Self::LlmGenerate { .. } => "llm_generate",
            Self::LlmChunk { .. } => "llm_chunk",
            Self::LlmDone { .. } => "llm_done",
            Self::EmbedEncode { .. } => "embed_encode",
            Self::EmbedResult { .. } => "embed_result",
            Self::TtsSynthesize { .. } => "tts_synthesize",
            Self::TtsResult { .. } => "tts_result",
            Self::SttTranscribe { .. } => "stt_transcribe",
            Self::SttResult { .. } => "stt_result",
            Self::ProviderListModels { .. } => "provider_list_models",
            Self::ProviderModels { .. } => "provider_models",
        }
    }
}
