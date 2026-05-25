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
//! When a [`MemoryStore`] is attached, tool definitions are embedded into vectors and stored
//! in the `tool_embeddings` table. [`CompositeToolRegistry::list_relevant_tools`] performs
//! cosine-similarity filtering based on the user's current query embedding.
#![warn(missing_docs)]

pub mod error;
pub mod ipc_client;
pub mod mcp_client;
pub mod tool_host_manager;
pub mod tools;
pub mod config;

pub use error::ToolError;
pub use ipc_client::IpcToolRegistry;
pub use mcp_client::McpToolRegistry;
pub use tool_host_manager::ToolHostManager;
pub use tools::{
    CompositeToolRegistry, ToolCategory, ToolDefinition, ToolRegistry, compute_tool_version_hash,
};
pub use config::{ToolSettings, ToolEntry, McpServerConfig, McpTransport};
