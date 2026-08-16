//! # ene-plugin-proto
//!
//! Unified plugin IPC protocol definitions and shared tool types.
//!
//! This crate defines the wire contract for both the *tool* IPC (v2) and the
//! richer *plugin* IPC (v3) that can provide tools **and** LLM/TTS/STT
//! providers simultaneously.
//!
//! ## What lives here
//!
//! ### Tool types
//!
//! - [`ToolSpec`] / [`ToolName`] / [`ToolRagProfile`] — LLM-facing tool
//!   schemas and RAG metadata.
//! - [`ToolError`] — structured, IPC-serializable tool error type.
//! - [`ToolProvider`] — trait implemented by each tool binary.
//! - [`IpcRequest`] / [`IpcResponse`] — tool IPC v2 wire messages.
//! - [`SandboxConfigData`] — sandbox configuration shared across tool processes.
//! - [`HostServiceId`] / [`HostServiceRequest`] / [`HostServiceResponse`] —
//!   multiplexed host-service channel (shared socket, passenger services).
//! - [`IpcStream`] / [`IpcListener`] / [`cleanup_path`] — cross-platform
//!   transport layer (UDS / Named Pipe).
//!
//! ### Plugin types (protocol v8)
//!
//! - [`PluginCapabilities`] — advertised during the handshake so the host can
//!   route tool registrations and provider factories.
//! - [`LlmProviderSpec`] / [`TtsProviderSpec`] / [`SttProviderSpec`] — provider
//!   descriptors carried inside [`PluginCapabilities`].
//! - [`PluginIpcRequest`] / [`PluginIpcResponse`] — the v3 wire messages,
//!   including streaming LLM messages (`CreateChatStream`, `StreamChunk`,
//!   `StreamEnd`, `StreamError`).
//! - [`read_plugin_request`] / [`write_plugin_request`] /
//!   [`read_plugin_response`] / [`write_plugin_response`] — framing helpers
//!   that reuse the same 4-byte little-endian length-prefixed pattern. The
//!   handshake exchange always uses JSON; frames after the handshake use the
//!   negotiated [`WireFormat`] (`MessagePack` for protocol v6+, JSON below).
//! - [`PluginError`] — the plugin crate's error type.
//!
//! ### Broker channel (protocol v8)
//!
//! - [`broker`] — `BrokerRequest` / `BrokerResponse`: the typed surface of
//!   the `Artifact` / `File` / `Network` / `Process` / `Credential` /
//!   `Platform` host-service passengers. Plugins have no direct OS access;
//!   every operation is mediated by the host through these messages.
//!
//! ## Crate boundaries
//!
//! Wire-protocol concerns only. Must not gain business logic, database
//! access, or AI-provider dependencies.
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        clippy::unwrap_used,
        reason = "unit tests use expect/unwrap/panic for concise assertions"
    )
)]

/// Broker-channel wire types (protocol v8).
pub mod broker;
pub mod capabilities;
pub use broker::{
    ArtifactInfo, BrokerErrorCode, BrokerHandler, BrokerRequest, BrokerResponse, BrokerSink,
    ConflictMode, FileEntry, HttpMethod, SharedBrokerHandler, WireArtifactKind,
};
pub use host_service::{read_framed_json, write_framed_json};
pub mod capability_service;
pub mod error;
pub mod host_service;
pub mod ipc;
pub mod sandbox;
pub mod tool_error;
pub mod tool_ipc;
pub mod tool_provider;
pub mod tool_types;
pub mod transport;
pub mod usage;
mod wire;
/// WebSocket broker passenger wire types (protocol v8).
pub mod ws;

pub use capabilities::{
    CapabilityParseError, CapabilityRef, CapabilityRequirement, ConcurrencyHint,
    DEFAULT_SAMPLE_RATE, LlmProviderSpec, PluginCapabilities, ResourceClass, SttProviderSpec,
    TtsProviderSpec, VadProviderSpec,
};
pub use capability_service::{
    CapabilityCall, CapabilityCallError, CapabilityCallErrorCode, CapabilityCallResult,
    CapabilityServiceHandler, CapabilityServiceRequest, CapabilityServiceResponse,
    read_capability_service_request, read_capability_service_response,
    write_capability_service_request, write_capability_service_response,
};
pub use error::{PluginError, ProviderErrorKind};
pub use host_service::{
    HOST_SERVICE_MAX_MESSAGE_SIZE, HostServiceErrorCode, HostServiceId, HostServiceRequest,
    HostServiceResponse, read_host_service_request, read_host_service_response,
    write_host_service_request, write_host_service_response,
};
pub use ipc::{
    ConfigFieldError, ConfigOption, PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION,
    PluginIpcRequest, PluginIpcResponse, VadEvent, VersionRange, read_plugin_request,
    read_plugin_response, write_plugin_request, write_plugin_response,
};
pub use sandbox::SandboxConfigData;
/// Error kind discriminator for [`ToolError::Generic`].
pub use tool_error::ErrorKind;
pub use tool_error::ToolError;
/// Interactive user input prompt (used inside [`ToolError::UserInputRequired`]).
pub use tool_error::{MultiAnswer, QuestionItem, UserInputPrompt};
pub use tool_ipc::{
    CallContext, DeferredStatus, IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, ToolConfigAccessor,
    read_ipc_request, read_ipc_response, write_ipc_request, write_ipc_response,
};
/// Outcome of a deferred (background) tool call.
pub use tool_provider::DeferredOutcome;
pub use tool_provider::ToolProvider;
pub use tool_types::{
    EmbeddingField, KeywordSet, Reversibility, SideEffects, ToolCategory, ToolContent, ToolExample,
    ToolName, ToolRagProfile, ToolResult, ToolSpec, ToolVersion, UndoMetadata,
};
pub use wire::WireFormat;

// Re-export the transport layer so downstream plugin crates only need to
// depend on `ene-plugin-proto` for the wire + transport.
pub use transport::{IpcListener, IpcStream, cleanup_path};
pub use usage::TokenUsage;
