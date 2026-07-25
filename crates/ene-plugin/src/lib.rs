//! # ene-plugin
//!
//! Plugin authoring facade for the ene unified plugin system.
//!
//! This crate provides the [`Plugin`] trait and [`run_plugin_server`] entry
//! point for building plugin binaries. A plugin can expose tools, LLM
//! providers (streaming and non-streaming), and embedding providers
//! simultaneously — the [`PluginCapabilities`] advertised during the IPC
//! handshake tells the host which registries to populate.
//!
//! ## Relationship to other crates
//!
//! - [`ene-plugin-proto`](ene_plugin_proto) — wire protocol definitions
//!   (protocol v3), framing helpers, and transport layer.
//! - `ene-plugin-host` — host-side process supervision and capability
//!   routing (consumes plugins, does not author them).
//! - [`ene-tool-proto`](ene_plugin_proto) — the underlying tool IPC (v2) that
//!   the plugin protocol extends.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use ene_plugin::{Plugin, PluginCapabilities, PluginError};
//!
//! struct MyPlugin;
//!
//! #[async_trait]
//! impl Plugin for MyPlugin {
//!     fn capabilities(&self) -> PluginCapabilities {
//!         PluginCapabilities::default()
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), PluginError> {
//!     ene_plugin::run_plugin_server(Box::new(MyPlugin)).await
//! }
//! ```
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

/// Compatibility adapter for wrapping `ToolProvider` as a `Plugin`.
pub mod compat;
/// Composite registry that aggregates multiple `ToolProvider` instances.
pub mod host_registry;
/// Plugin trait and streaming chunk types.
pub mod plugin;
/// Plugin IPC server and dispatch loop.
pub mod server;
/// Server helper for running a tool provider over IPC.
pub mod tool_server;

pub use compat::ToolPluginAdapter;
pub use host_registry::HostRegistry;
pub use plugin::{Plugin, PluginStream, PluginStreamChunk};
pub use server::run_plugin_server;
pub use tool_server::run_tool_server;

// Re-export key types from ene-plugin-proto so plugin authors only need
// to depend on `ene-plugin` for the full authoring surface.
/// Cross-platform IPC transport (re-exported from `ene-plugin-proto`).
pub use ene_plugin_proto::{IpcListener, IpcStream, cleanup_path};
pub use ene_plugin_proto::{
    LlmProviderSpec, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginError,
    PluginIpcRequest, PluginIpcResponse, SttProviderSpec, TtsProviderSpec,
};
/// Shared tool types (re-exported from `ene-plugin-proto`).
pub use ene_plugin_proto::{ToolError, ToolSpec};

// Re-export additional tool-proto types used by the Plugin trait and server.
pub use ene_plugin_proto::{DeferredStatus, SandboxConfigData};
