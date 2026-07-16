use thiserror::Error;

/// Error types for the tool host subsystem.
#[derive(Debug, Error)]
pub enum EneToolHostError {
    /// Error originating from the underlying tool protocol (IPC).
    #[error(transparent)]
    Protocol(#[from] ene_tool_proto::ToolError),
    /// Configuration error (e.g. invalid RAG config).
    #[error("Configuration error: {0}")]
    Config(String),
    /// I/O error during tool spawning or socket management.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Execution failed (e.g. tool binary not found).
    #[error("Execution failed: {message}")]
    ExecutionFailed {
        /// A descriptive message about the failure.
        message: String,
    },
    /// A prefixed fallback name also collided in a composite registry.
    ///
    /// [`CompositeToolRegistry`] resolves primary collisions by prefix-renaming
    /// (`"<idx>:<name>"`). This error fires only when that prefixed name
    /// itself collides — a near-impossible edge case. Primary collisions
    /// in [`HostRegistry`] produce [`ToolError::DuplicateName`] directly
    /// (hard error, no fallback).
    #[error("Duplicate tool name: {tool_name}")]
    DuplicateToolName {
        /// Colliding tool name.
        tool_name: String,
    },

    // ── MCP-specific errors ──────────────────────────────────────────
    /// MCP server process could not be spawned or connected to.
    #[error("MCP Connect error: {0}")]
    McpConnect(String),
    /// MCP handshake failed (protocol version mismatch, etc.).
    #[error("MCP Handshake error: {0}")]
    McpHandshake(String),
    /// MCP RPC call failed (e.g. method not found, server error).
    #[error("MCP RPC error: {0}")]
    McpRpc(String),
    /// MCP server advertised an invalid tool name.
    #[error("MCP invalid tool name: {0}")]
    McpInvalidName(String),
}

impl EneToolHostError {
    /// Creates a protocol `IpcClient` error with the given message.
    pub fn ipc(message: impl Into<String>) -> Self {
        Self::Protocol(ene_tool_proto::ToolError::IpcClient {
            message: message.into(),
        })
    }
}

/// Alias for tool host error.
pub type ToolHostError = EneToolHostError;
