//! Plugin IPC: independent `core` / `tool` / `provider` / `capability`
//! subprotocols (D-22).
//!
//! Frames are 32-bit big-endian length + `MessagePack`. `id` is required on
//! every request/response (never defaulted). Bulk payloads leave the frame
//! via `capability` grants and Unix `SCM_RIGHTS` (or a Windows handle grant).

#![cfg_attr(
    test,
    expect(clippy::unwrap_used, clippy::expect_used, reason = "tests fail fast")
)]
#![deny(unsafe_code)]

mod broker;
#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "SCM_RIGHTS sendmsg/recvmsg is the Unix bulk-FD path"
)]
mod bulk;
#[cfg(not(unix))]
mod bulk;
mod config;
mod dispatch;
mod error;
mod frame;
mod host;
mod plugin;
mod protocol;
mod provider;

pub use broker::{
    BrokerClient, BrokerClientTransport, BrokerErrorCode, BrokerRequest, BrokerResponse,
    BrokerSession, read_broker_request, read_broker_response, write_broker_request,
    write_broker_response,
};
#[cfg(not(unix))]
pub use bulk::should_spill;
#[cfg(unix)]
pub use bulk::{recv_fds, send_fds, should_spill};
pub use config::{
    PluginConfigApplyResult, PluginConfigError, PluginConfigOption, PluginConfigOptionsResult,
    PluginConfigSchema, PluginConfigValidateResult, redact_config_values, scrub_schema_secrets,
    secret_keys_from_schema,
};
pub use dispatch::{
    AssetsHandler, EmbedHandler, LlmHandler, ModelsHandler, PluginIdentity, ProviderHandlers,
    SttHandler, TtsHandler, serve_provider, serve_provider_from_env,
};
pub use error::IpcError;
pub use frame::{MAX_FRAME_BYTES, frame_limit, read_frame, write_frame};
pub use host::{HostConn, negotiate};
pub use plugin::{BuiltinKind, ToolHandler, serve_from_env, serve_plugin};
pub use protocol::{
    ApprovalAnswer, ApprovalQuery, BulkRef, CAPABILITY_VERSION, CORE_VERSION, CapabilityDenied,
    CapabilityGrant, CapabilityGranted, CapabilityRelease, CapabilityRequest,
    DEFAULT_BULK_THRESHOLD_BYTES, FlowControl, HostHello, Negotiated, ProtoId, ProtocolRanges,
    StreamOpen, StreamOpened, TOOL_VERSION, ToolCall, ToolResult, ToolSpecWire, VersionRange,
};
pub use provider::{
    AssetVersionView, AssetView, EmbedRequest, EmbedResult, InstallAssetRequest,
    InstallAssetResult, InstallPhase, InstallStatusRequest, InstallStatusResult, ListAssetsResult,
    ListModelsRequest, ListModelsResult, LlmChunk, LlmGenerateRequest, LlmGeneration, LlmImage,
    LlmInnerLine, LlmMessage, LlmRole, LlmToolCall, LlmToolSchema, PROVIDER_ASSETS_VERSION,
    PROVIDER_EMBED_VERSION, PROVIDER_LLM_VERSION, PROVIDER_MODELS_VERSION, PROVIDER_STT_VERSION,
    PROVIDER_TTS_VERSION, PROVIDER_VERSION, ProviderAuth, ProviderFaces, SetActiveAssetRequest,
    SetActiveAssetResult, SttRequest, SttResult, TtsAudio, TtsRequest,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_asset_version;
#[cfg(test)]
mod tests_capability;
#[cfg(test)]
mod tests_config;
