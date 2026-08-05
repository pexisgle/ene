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
//! ### Plugin types (protocol v6)
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
//!   negotiated [`WireFormat`] (`MessagePack` for protocol v6, JSON below).
//! - [`PluginError`] — the plugin crate's error type.
//!
//! ## Crate boundaries
//!
//! Wire-protocol concerns only. Must not gain business logic, database
//! access, or AI-provider dependencies.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        clippy::unwrap_used,
        reason = "unit tests use expect/unwrap/panic for concise assertions"
    )
)]

/// Plugin capability declarations.
pub mod capabilities;
/// Capability-call wire types and the host-service `capability` passenger.
pub mod capability_service;
/// Plugin error types.
pub mod error;
/// Multiplexed host-service channel wire types.
pub mod host_service;
/// Plugin IPC wire protocol (request / response / framing).
pub mod ipc;
/// Sandbox configuration types.
pub mod sandbox;
/// Tool error types.
pub mod tool_error;
/// Tool IPC wire protocol (request / response / framing).
pub mod tool_ipc;
/// Tool provider trait and deferred outcome types.
pub mod tool_provider;
/// Shared tool types (`ToolCategory`, `ToolSpec`, etc.).
pub mod tool_types;
/// UDS / Named Pipe transport layer.
pub mod transport;
/// Token usage accounting for LLM responses.
pub mod usage;
mod wire;

/// Capability and provider spec types.
pub use capabilities::{
    CapabilityParseError, CapabilityRef, CapabilityRequirement, ConcurrencyHint, LlmProviderSpec,
    PluginCapabilities, ResourceClass, SttProviderSpec, TtsProviderSpec,
};
/// Capability-call wire types, framing helpers, and the passenger interface.
pub use capability_service::{
    CapabilityCall, CapabilityCallError, CapabilityCallErrorCode, CapabilityCallResult,
    CapabilityServiceHandler, CapabilityServiceRequest, CapabilityServiceResponse,
    read_capability_service_request, read_capability_service_response,
    write_capability_service_request, write_capability_service_response,
};
/// Plugin error type.
pub use error::{PluginError, ProviderErrorKind};
/// Host-service channel types and framing helpers.
pub use host_service::{
    HOST_SERVICE_MAX_MESSAGE_SIZE, HostServiceErrorCode, HostServiceId, HostServiceRequest,
    HostServiceResponse, read_host_service_request, read_host_service_response,
    write_host_service_request, write_host_service_response,
};
/// Plugin IPC message types, protocol version, and framing helpers.
pub use ipc::{
    ConfigFieldError, ConfigOption, PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION,
    PluginIpcRequest, PluginIpcResponse, VersionRange, read_plugin_request, read_plugin_response,
    write_plugin_request, write_plugin_response,
};
/// Sandbox configuration data sent from the host.
pub use sandbox::SandboxConfigData;
/// Error kind discriminator for [`ToolError::Generic`].
pub use tool_error::ErrorKind;
/// Tool error type.
pub use tool_error::ToolError;
/// Interactive user input prompt (used inside [`ToolError::UserInputRequired`]).
pub use tool_error::{MultiAnswer, QuestionItem, UserInputPrompt};
/// Tool IPC message types and serialisation helpers.
pub use tool_ipc::{
    CallContext, DeferredStatus, IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, ToolConfigAccessor,
    read_ipc_request, read_ipc_response, write_ipc_request, write_ipc_response,
};
/// Outcome of a deferred (background) tool call.
pub use tool_provider::DeferredOutcome;
/// Tool provider trait.
pub use tool_provider::ToolProvider;
/// Shared tool types.
pub use tool_types::{
    EmbeddingField, KeywordSet, Reversibility, SideEffects, ToolCategory, ToolContent, ToolExample,
    ToolName, ToolRagProfile, ToolResult, ToolSpec, ToolVersion, UndoMetadata,
};
/// Negotiated payload encoding for plugin IPC frames.
pub use wire::WireFormat;

// Re-export the transport layer so downstream plugin crates only need to
// depend on `ene-plugin-proto` for the wire + transport.
/// Cross-platform IPC stream.
pub use transport::{IpcListener, IpcStream, cleanup_path};
/// Token usage accounting.
pub use usage::TokenUsage;
