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
    /// The circuit breaker is open after consecutive failures; the call was
    /// rejected without reaching the tool (#238).
    #[error(
        "Circuit breaker open for tool '{tool}' after {consecutive_failures} consecutive failures; call paused"
    )]
    CircuitOpen {
        /// The tool whose breaker is open.
        tool: String,
        /// Consecutive failure count that tripped the breaker.
        consecutive_failures: u32,
    },
    /// A tool name collision in [`CompositeToolRegistry`].
    ///
    /// Per API v1 / #135, name collision is a hard error at every
    /// registry layer. [`HostRegistry`] returns
    /// [`ToolError::DuplicateName`]; this variant covers the
    /// composite-level case.
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
        Self::Protocol(ene_tool_proto::ToolError::ipc_client(message))
    }
}

/// Alias for tool host error.
pub type ToolHostError = EneToolHostError;
