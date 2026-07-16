//! # ene-tool-host
//!
//! Tool process lifecycle manager for the ene AI character platform.
//!
//! Spawns, supervises, and auto-restarts tool binaries over IPC (Unix Domain Sockets / Windows Named Pipes).
//! Also manages external MCP (Model Context Protocol) server connections.
//!
//! ## Key Types
//!
//! - [`ToolHostManager`] — Orchestrates all tool processes: discovery, spawning, health monitoring
//! - [`IpcToolRegistry`] — IPC client to a single tool binary with auto-reconnect
//! - [`McpToolRegistry`] — Connects to MCP servers via stdio or HTTP transport
//! - [`CompositeToolRegistry`] — Aggregates multiple registries with RAG-based tool filtering
//! - [`ToolRegistry`] trait — Unified interface for listing and calling tools
//!
//! ## Crash Resilience
//!
//! Both the process layer ([`ToolHostManager`]) and the IPC layer ([`IpcToolRegistry`]) implement
//! exponential backoff with a maximum of 5 retries (500ms → 30s) on process death or connection loss.
//!
//! ## Tool RAG
//!
//! Tool RAG is handled by the [`ene_tool_rag`] crate. See its documentation for the
//! multi-vector embedding pipeline, `HyDE`, and LLM rerank details.
#![warn(missing_docs)]

/// Tool and MCP configuration types.
pub mod config;
/// Tool host error types
pub mod error;
/// IPC client with auto-reconnect for tool binaries.
pub mod ipc_registry;
/// MCP server configuration types.
pub mod mcp_config;
/// MCP client for external server connections.
pub mod mcp_registry;
/// Tool process lifecycle manager.
pub mod tool_host_manager;
/// Tool registry types (Composite, `ToolRegistry` trait, etc.).
pub mod tools;

/// Tool configuration types.
pub use config::{ToolConfig, ToolEntry};
/// `ToolSpec` type (re-exported from `ene-tool-proto`).
#[doc(no_inline)]
pub use ene_tool_proto::ToolSpec;
/// Tool host error types
pub use error::{EneToolHostError, ToolHostError};
/// IPC client with auto-reconnect.
pub use ipc_registry::IpcToolRegistry;
/// MCP server configuration types.
pub use mcp_config::{McpServerConfig, McpTransport};
/// MCP client for external servers.
pub use mcp_registry::McpToolRegistry;
/// Tool process lifecycle manager.
pub use tool_host_manager::ToolHostManager;
/// Registry types and the `ToolRegistry` trait.
pub use tools::{CompositeToolRegistry, ToolCategory, ToolRegistry, compute_tool_version_hash};
