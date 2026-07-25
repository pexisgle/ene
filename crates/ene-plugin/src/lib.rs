//! # ene-plugin
//!
//! Plugin authoring facade for the ene unified plugin system.
//!
//! This crate provides the [`ToolPlugin`], [`LlmPlugin`], and [`EmbedPlugin`]
//! traits and the [`run_plugin_server`] entry point for building plugin
//! binaries. A plugin can implement any subset of these traits; the server
//! dispatches requests to the appropriate trait via [`PluginDispatch`].
//!
//! ## Relationship to other crates
//!
//! - [`ene-plugin-proto`](ene_plugin_proto) — wire protocol definitions
//!   (protocol v4), framing helpers, and transport layer.
//! - `ene-plugin-host` — host-side process supervision and capability
//!   routing (consumes plugins, does not author them).
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use std::sync::Arc;
//! use ene_plugin::{ToolPlugin, ToolPluginCapabilities, PluginDispatch, PluginError};
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl ToolPlugin for MyTool {
//!     fn tool_capabilities(&self) -> ToolPluginCapabilities {
//!         ToolPluginCapabilities { tool_count: 0 }
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), PluginError> {
//!     ene_plugin::run_plugin_server(
//!         PluginDispatch::new(Some(Arc::new(MyTool)), None, None, None, None),
//!     ).await
//! }
//! ```
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

/// Compatibility adapter for wrapping legacy [`ToolProvider`] as [`ToolPlugin`].
pub mod compat;
/// Composite registry that aggregates multiple `ToolProvider` instances.
pub mod host_registry;
/// Plugin trait and streaming chunk types.
pub mod plugin;
/// Plugin IPC server and dispatch loop.
pub mod server;
/// Server helper for running a tool provider over IPC.
pub mod tool_server;

pub use compat::ToolProviderPlugin;
pub use host_registry::HostRegistry;
pub use plugin::{
    EmbedPlugin, LlmPlugin, PluginStream, PluginStreamChunk, SttPlugin, ToolPlugin,
    ToolPluginCapabilities, TtsPlugin,
};
pub use server::{PluginDispatch, run_plugin_server};

// Re-export key types from ene-plugin-proto so plugin authors only need
// to depend on `ene-plugin` for the full authoring surface.
/// Cross-platform IPC transport (re-exported from `ene-plugin-proto`).
pub use ene_plugin_proto::{IpcListener, IpcStream, cleanup_path};
pub use ene_plugin_proto::{
    LlmProviderSpec, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginError,
    PluginIpcRequest, PluginIpcResponse, SttProviderSpec, TtsProviderSpec, VersionRange,
};
/// Shared tool types (re-exported from `ene-plugin-proto`).
pub use ene_plugin_proto::{ToolError, ToolResult, ToolSpec};

// Re-export additional tool-proto types used by the server.
pub use ene_plugin_proto::{DeferredStatus, SandboxConfigData};
