//! Plugin IPC: independent `core` / `tool` / `provider` subprotocols (D-22).
//!
//! Frames are 32-bit big-endian length + `MessagePack`. `id` is required on
//! every request/response (never defaulted).

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

mod dispatch;
mod error;
mod frame;
mod host;
mod plugin;
mod protocol;
mod provider;

pub use dispatch::{
    EmbedHandler, LlmHandler, ModelsHandler, PluginIdentity, ProviderHandlers, SttHandler,
    TtsHandler, serve_provider, serve_provider_from_env,
};
pub use error::IpcError;
pub use frame::{MAX_FRAME_BYTES, frame_limit, read_frame, write_frame};
pub use host::{HostConn, negotiate};
pub use plugin::{BuiltinKind, ToolHandler, serve_from_env, serve_plugin};
pub use protocol::{
    CORE_VERSION, HostHello, Negotiated, ProtoId, ProtocolRanges, TOOL_VERSION, ToolCall,
    ToolResult, ToolSpecWire, VersionRange,
};
pub use provider::{
    EmbedRequest, EmbedResult, ListModelsRequest, ListModelsResult, LlmChunk, LlmGenerateRequest,
    LlmGeneration, LlmImage, LlmInnerLine, LlmMessage, LlmRole, LlmToolCall, LlmToolSchema,
    PROVIDER_EMBED_VERSION, PROVIDER_LLM_VERSION, PROVIDER_MODELS_VERSION, PROVIDER_STT_VERSION,
    PROVIDER_TTS_VERSION, PROVIDER_VERSION, ProviderAuth, ProviderFaces, SttRequest, SttResult,
    TtsAudio, TtsRequest,
};

#[cfg(test)]
mod tests;
