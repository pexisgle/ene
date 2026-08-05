//! # ene-plugin-host
//!
//! Host-side plugin process management for the ene unified plugin system.
//!
//! This crate provides [`PluginHostManager`], which discovers, spawns, and
//! supervises plugin binaries (protocol v6), routing their advertised
//! capabilities into the host's tool and LLM provider registries. It also
//! owns the [`ToolRegistry`] trait and [`CompositeToolRegistry`] that
//! aggregate plugin-provided and MCP tools.
//!
//! ## Key types
//!
//! - [`PluginHostManager`] — orchestrates plugin lifecycle: discovery,
//!   spawning, handshake, capability routing, health probes, and shutdown.
//! - [`ToolRegistry`] — unified interface for listing and calling tools.
//! - [`CompositeToolRegistry`] — aggregates multiple registries.
//! - [`IpcPluginConnection`] — IPC client to a single plugin binary with
//!   handshake, request/response, and streaming support.
//! - [`IpcLlmProvider`] — bridges the plugin IPC streaming protocol to the
//!   [`ene_ai::LlmProvider`] trait.
//! - [`IpcLlmProviderFactory`] — implements [`ene_ai::LlmProviderFactory`]
//!   so plugin-provided LLM providers integrate with the global
//!   [`LlmProviderRegistry`](ene_ai::LlmProviderRegistry).
//! - [`IpcTtsProvider`] / [`IpcTtsProviderFactory`] — the TTS counterparts,
//!   integrating plugin-provided TTS providers with
//!   [`AudioProviderRegistry`](ene_ai::AudioProviderRegistry).
//!
//! ## Relationship to other crates
//!
//! - [`ene-plugin-proto`](ene_plugin_proto) — wire protocol definitions
//!   (tool IPC v2 + plugin protocol v6), framing helpers, and transport layer.
//! - [`ene-plugin`](ene_plugin) — plugin authoring facade (used by plugin
//!   binaries, not by the host).
//! - [`ene-ai`](ene_ai) — `LlmProvider` / `LlmProviderFactory` traits that
//!   [`IpcLlmProvider`] / [`IpcLlmProviderFactory`] implement, and the
//!   `TtsProvider` / `TtsProviderFactory` traits that [`IpcTtsProvider`] /
//!   [`IpcTtsProviderFactory`] implement.
#![warn(missing_docs)]

/// Host-side registry of plugin capability declarations and resolution.
pub mod capability_registry;
/// Host mediation for plugin-to-plugin capability calls.
pub mod capability_service;
/// Per-plugin circuit breaker for consecutive-failure fail-fast.
pub mod circuit_breaker;
/// Plugin system configuration section.
pub mod config;
/// Host-side registry of per-plugin credential declarations.
pub mod credential_registry;
/// IPC-backed embedding provider bridging to `ene_ai::EmbeddingProvider`.
pub mod embedding;
/// Plugin host error types.
pub mod error;
/// LLM provider factory backed by a plugin IPC connection.
pub mod factory;
/// Plugin health events surfaced to diagnostics.
pub mod health;
/// IPC client to a single plugin binary.
pub mod ipc_plugin;
/// IPC-backed LLM provider bridging to `ene_ai::LlmProvider`.
pub mod ipc_provider;
/// IPC-backed TTS provider bridging to `ene_ai::TtsProvider`.
pub mod ipc_tts;
/// Plugin host manager: process supervision and capability routing.
pub mod manager;
/// MCP server configuration types.
pub mod mcp_config;
/// MCP client for external server connections.
pub mod mcp_registry;
/// Redaction of plugin configuration values at the host boundary.
pub mod redact;
/// Tool registry trait, composite registry, and deferred call types.
pub mod tool_registry;
/// TTS provider factory backed by a plugin IPC connection.
pub mod tts_factory;
/// RIFF/WAVE decoder for plugin-delivered TTS audio.
pub mod wav;

/// Capability registry and requirement gate.
pub use capability_registry::{
    CapabilityDeclaration, CapabilityRegistry, PluginCapabilityDeclarations,
    evaluate_capability_gate,
};
/// Capability call mediation.
pub use capability_service::{
    CapabilityCallHandler, CapabilityMediator, ManagerCapabilityHandler,
    resolve_capability_provider,
};
/// Per-plugin circuit breaker.
pub use circuit_breaker::{BreakerState, CircuitBreaker};
/// Plugin system configuration.
pub use config::{PluginConfig, PluginEntry};
/// Host-side registry of per-plugin credential declarations.
pub use credential_registry::CredentialRegistry;
/// IPC-backed embedding provider and factory.
pub use embedding::{IpcEmbeddingProvider, IpcEmbeddingProviderFactory};
/// Plugin host error type.
pub use error::PluginHostError;
/// Backward-compatible alias for [`PluginHostError`].
pub use error::ToolHostError;
/// LLM provider factory for plugin-provided providers.
pub use factory::IpcLlmProviderFactory;
/// Plugin health events.
pub use health::{DisabledReason, PluginHealthEvent};
/// IPC connection to a single plugin binary.
pub use ipc_plugin::{IpcPluginConnection, SetConfigOutcome};
/// IPC-backed LLM provider.
pub use ipc_provider::IpcLlmProvider;
/// IPC-backed TTS provider.
pub use ipc_tts::IpcTtsProvider;
/// Plugin host manager.
pub use manager::{
    EmbeddingFactoriesByPlugin, EmbeddingFactoryHandle, LlmFactoriesByPlugin, LlmFactoryHandle,
    PluginHostManager, TtsFactoriesByPlugin, TtsFactoryHandle,
};
/// MCP server configuration types.
pub use mcp_config::{McpServerConfig, McpTransport};
/// MCP client for external servers.
pub use mcp_registry::McpToolRegistry;
/// Config redaction helpers (see [`redact`]).
pub use redact::{redact_config, redact_config_unschematized};
/// Tool registry trait, composite registry, and deferred call types.
pub use tool_registry::{
    CompositeToolRegistry, DeferredCallResult, ToolRegistry, compute_tool_version_hash,
};
/// TTS provider factory for plugin-provided providers.
pub use tts_factory::IpcTtsProviderFactory;
