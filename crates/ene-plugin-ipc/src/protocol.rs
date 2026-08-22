use serde::{Deserialize, Serialize};

use crate::provider::{
    EmbedRequest, EmbedResult, InstallAssetRequest, InstallAssetResult, InstallStatusRequest,
    InstallStatusResult, ListAssetsResult, ListModelsRequest, ListModelsResult, LlmChunk,
    LlmGenerateRequest, LlmGeneration, ProviderFaces, SetActiveAssetRequest, SetActiveAssetResult,
    SttRequest, SttResult, TtsAudio, TtsRequest,
};

/// Current `core` subprotocol version.
pub const CORE_VERSION: u32 = 1;
/// Current `tool` subprotocol version.
pub const TOOL_VERSION: u32 = 1;
/// Current `capability` subprotocol version.
pub const CAPABILITY_VERSION: u32 = 1;
/// Default `plugins.ipc.bulk_threshold_bytes`. `0` in config keeps this.
pub const DEFAULT_BULK_THRESHOLD_BYTES: u32 = 65_536;

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
            capability: Some(VersionRange::exact(CAPABILITY_VERSION)),
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capability: Option<u32>,
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
    /// When false the host never sends config RPCs; default handlers are unused.
    #[serde(default)]
    pub has_config: bool,
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
    /// Unix socket path where the host serves broker RPCs for this plugin.
    #[serde(default)]
    pub broker_socket: Option<String>,

    /// Implementation-agnostic grouping label for discovery.
    #[serde(default)]
    pub category: String,
    /// Extra search terms beyond name and description.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Example invocations or use cases for discovery.
    #[serde(default)]
    pub examples: Vec<String>,
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

/// Plugin → host Broker RPC (`capability.request`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub method: String,
    pub params: serde_json::Value,
    pub capability_ref: String,
}

/// Host → plugin resource grant. Unix follows this frame with `fd_count`
/// `SCM_RIGHTS` descriptors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub grant_id: String,
    pub method: String,
    #[serde(default)]
    pub fd_count: u32,
    #[serde(default)]
    pub stream_id: String,
}

/// Plugin ack of a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGranted {
    pub grant_id: String,
    pub status: String,
}

/// Host denial of a Broker request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDenied {
    pub method: String,
    pub reason: String,
}

/// Either side releases a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRelease {
    pub grant_id: String,
}

/// Plugin → host: is this operation already approved?
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalQuery {
    pub tool: String,
    pub target: String,
}

/// Host → plugin approval answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAnswer {
    pub allowed: bool,
    pub reason: String,
}

/// Host → plugin: open an out-of-band bulk stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOpen {
    pub stream_id: String,
    pub kind: String,
}

/// Plugin ack. `fd_count` is 1 when a socketpair fd follows on Unix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOpened {
    pub stream_id: String,
    #[serde(default)]
    pub fd_count: u32,
}

/// Back-pressure on a bulk stream. Notification: no `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowControl {
    pub stream_id: String,
    pub pause: bool,
}

/// Frame-side reference to bytes that travelled out of band.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkRef {
    pub stream_id: String,
    pub byte_len: u64,
}

/// Envelope. `id` is required on request/response variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    Hello {
        body: HostHello,
    },
    HelloAck {
        body: HelloAck,
    },
    HelloReject {
        body: HelloReject,
    },
    Ping {
        id: u64,
    },
    Pong {
        id: u64,
    },
    ToolList {
        id: u64,
    },
    ToolSpec {
        id: u64,
        tools: Vec<ToolSpecWire>,
    },
    ToolCall {
        id: u64,
        body: ToolCall,
    },
    ToolResult {
        id: u64,
        body: ToolResult,
    },
    ToolCancel {
        id: u64,
        call_id: String,
    },
    Shutdown {
        id: u64,
    },
    Drain {
        id: u64,
    },
    DrainAck {
        id: u64,
    },
    Log {
        fields: serde_json::Value,
    },
    LlmGenerate {
        id: u64,
        body: LlmGenerateRequest,
    },
    LlmChunk {
        id: u64,
        body: LlmChunk,
    },
    LlmDone {
        id: u64,
        body: LlmGeneration,
    },
    EmbedEncode {
        id: u64,
        body: EmbedRequest,
    },
    EmbedResult {
        id: u64,
        body: EmbedResult,
    },
    TtsSynthesize {
        id: u64,
        body: TtsRequest,
    },
    TtsResult {
        id: u64,
        body: TtsAudio,
    },
    SttTranscribe {
        id: u64,
        body: SttRequest,
    },
    SttResult {
        id: u64,
        body: SttResult,
    },
    ProviderListModels {
        id: u64,
        body: ListModelsRequest,
    },
    ProviderModels {
        id: u64,
        body: ListModelsResult,
    },
    ProviderListAssets {
        id: u64,
    },
    ProviderAssets {
        id: u64,
        body: ListAssetsResult,
    },
    ProviderInstallAsset {
        id: u64,
        body: InstallAssetRequest,
    },
    ProviderInstallAssetAck {
        id: u64,
        body: InstallAssetResult,
    },
    ProviderInstallStatus {
        id: u64,
        body: InstallStatusRequest,
    },
    ProviderInstallStatusResult {
        id: u64,
        body: InstallStatusResult,
    },
    ProviderSetActiveAsset {
        id: u64,
        body: SetActiveAssetRequest,
    },
    ProviderSetActiveAssetResult {
        id: u64,
        body: SetActiveAssetResult,
    },
    CapabilityRequest {
        id: u64,
        body: CapabilityRequest,
    },
    CapabilityGrant {
        id: u64,
        body: CapabilityGrant,
    },
    CapabilityGranted {
        id: u64,
        body: CapabilityGranted,
    },
    CapabilityDenied {
        id: u64,
        body: CapabilityDenied,
    },
    CapabilityRelease {
        id: u64,
        body: CapabilityRelease,
    },
    CapabilityReleased {
        id: u64,
    },
    CapabilityApprovalQuery {
        id: u64,
        body: ApprovalQuery,
    },
    CapabilityApproval {
        id: u64,
        body: ApprovalAnswer,
    },
    StreamOpen {
        id: u64,
        body: StreamOpen,
    },
    StreamOpened {
        id: u64,
        body: StreamOpened,
    },
    FlowControl {
        body: FlowControl,
    },
    PluginConfigSchema {
        id: u64,
    },
    PluginConfigSchemaResult {
        id: u64,
        body: crate::config::PluginConfigSchema,
    },
    PluginConfigValidate {
        id: u64,
        values: serde_json::Value,
    },
    PluginConfigValidateResult {
        id: u64,
        body: crate::config::PluginConfigValidateResult,
    },
    PluginConfigOptions {
        id: u64,
        field: String,
    },
    PluginConfigOptionsResult {
        id: u64,
        body: crate::config::PluginConfigOptionsResult,
    },
    PluginConfigApply {
        id: u64,
        values: serde_json::Value,
    },
    PluginConfigApplyResult {
        id: u64,
        body: crate::config::PluginConfigApplyResult,
    },
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
            Self::ProviderListAssets { .. } => "provider_list_assets",
            Self::ProviderAssets { .. } => "provider_assets",
            Self::ProviderInstallAsset { .. } => "provider_install_asset",
            Self::ProviderInstallAssetAck { .. } => "provider_install_asset_ack",
            Self::ProviderInstallStatus { .. } => "provider_install_status",
            Self::ProviderInstallStatusResult { .. } => "provider_install_status_result",
            Self::ProviderSetActiveAsset { .. } => "provider_set_active_asset",
            Self::ProviderSetActiveAssetResult { .. } => "provider_set_active_asset_result",
            Self::CapabilityRequest { .. } => "capability_request",
            Self::CapabilityGrant { .. } => "capability_grant",
            Self::CapabilityGranted { .. } => "capability_granted",
            Self::CapabilityDenied { .. } => "capability_denied",
            Self::CapabilityRelease { .. } => "capability_release",
            Self::CapabilityReleased { .. } => "capability_released",
            Self::CapabilityApprovalQuery { .. } => "capability_approval_query",
            Self::CapabilityApproval { .. } => "capability_approval",
            Self::StreamOpen { .. } => "stream_open",
            Self::StreamOpened { .. } => "stream_opened",
            Self::FlowControl { .. } => "flow_control",
            Self::PluginConfigSchema { .. } => "plugin_config_schema",
            Self::PluginConfigSchemaResult { .. } => "plugin_config_schema_result",
            Self::PluginConfigValidate { .. } => "plugin_config_validate",
            Self::PluginConfigValidateResult { .. } => "plugin_config_validate_result",
            Self::PluginConfigOptions { .. } => "plugin_config_options",
            Self::PluginConfigOptionsResult { .. } => "plugin_config_options_result",
            Self::PluginConfigApply { .. } => "plugin_config_apply",
            Self::PluginConfigApplyResult { .. } => "plugin_config_apply_result",
        }
    }
}
