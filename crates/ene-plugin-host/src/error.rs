use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginHostError {
    #[error("failed to spawn plugin '{name}': {reason}")]
    SpawnFailed { name: String, reason: String },

    #[error("failed to connect to plugin '{name}': {reason}")]
    ConnectFailed { name: String, reason: String },

    #[error("handshake failed for plugin '{name}': {reason}")]
    HandshakeFailed { name: String, reason: String },

    /// Carries the host's full supported range (not just a single expected
    /// value) alongside the version the plugin reported, so the diagnostic
    /// makes clear e.g. "host supports 3..=4, plugin acknowledged 2" instead
    /// of a bare version mismatch.
    #[error(
        "protocol version mismatch for plugin '{name}': host supports {host_min}..={host_max}, plugin acknowledged {got}"
    )]
    ProtocolMismatch {
        name: String,
        /// Minimum protocol version the host supports (inclusive).
        host_min: u32,
        /// Maximum protocol version the host supports (inclusive), i.e.
        /// [`PLUGIN_IPC_PROTOCOL_VERSION`](ene_plugin_proto::PLUGIN_IPC_PROTOCOL_VERSION).
        host_max: u32,
        /// The version the plugin acknowledged in `HandshakeAck`.
        got: u32,
    },

    #[error("duplicate LLM provider kind '{kind}' (already provided by '{existing_plugin}')")]
    DuplicateProvider {
        kind: String,
        existing_plugin: String,
    },

    #[error("plugin execution failed: {message}")]
    ExecutionFailed { message: String },

    /// A transport-level failure on the IPC connection (broken pipe,
    /// connection reset, EOF, or a missing stream).
    ///
    /// Deliberately distinct from [`ExecutionFailed`](Self::ExecutionFailed)
    /// so the connection layer can transparently reconnect and retry a
    /// request/response round-trip once (see
    /// [`IpcPluginConnection`](crate::IpcPluginConnection)). Timeouts are
    /// **not** transport failures — a hung plugin may still be processing a
    /// non-idempotent call, so timed-out requests are surfaced as
    /// [`ExecutionFailed`](Self::ExecutionFailed) and never auto-retried.
    #[error("plugin transport failed: {message}")]
    TransportFailed { message: String },

    #[error(
        "circuit breaker open for tool '{tool}' after {consecutive_failures} consecutive failures; call paused"
    )]
    CircuitOpen {
        tool: String,
        consecutive_failures: u32,
    },

    #[error(transparent)]
    Protocol(#[from] ene_plugin_proto::ToolError),

    /// Per API v1, name collision is a hard error at every
    /// registry layer.
    #[error("Duplicate tool name: {tool_name}")]
    DuplicateToolName { tool_name: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("MCP Connect error: {0}")]
    McpConnect(String),
    #[error("MCP Handshake error: {0}")]
    McpHandshake(String),
    #[error("MCP RPC error: {0}")]
    McpRpc(String),
    #[error("MCP Invalid tool name: {0}")]
    McpInvalidName(String),

    #[error("checksum mismatch for plugin '{name}': expected {expected}, got {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
}

impl PluginHostError {
    pub fn execution(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::TransportFailed {
            message: message.into(),
        }
    }

    pub fn ipc(message: impl Into<String>) -> Self {
        Self::Protocol(ene_plugin_proto::ToolError::ipc_client(message))
    }
}

/// Alias for backward compatibility with code that used `ToolHostError`.
pub type ToolHostError = PluginHostError;
