//! Plugin host error types.

use thiserror::Error;

/// Errors returned by the plugin host subsystem.
#[derive(Debug, Error)]
pub enum PluginHostError {
    /// Failed to spawn the plugin binary as a child process.
    #[error("failed to spawn plugin '{name}': {reason}")]
    SpawnFailed {
        /// The plugin name that failed to spawn.
        name: String,
        /// Underlying reason.
        reason: String,
    },

    /// Failed to establish an IPC connection to the plugin.
    #[error("failed to connect to plugin '{name}': {reason}")]
    ConnectFailed {
        /// The plugin name.
        name: String,
        /// Underlying reason.
        reason: String,
    },

    /// The plugin handshake failed (unexpected response, error message, etc.).
    #[error("handshake failed for plugin '{name}': {reason}")]
    HandshakeFailed {
        /// The plugin name.
        name: String,
        /// Underlying reason.
        reason: String,
    },

    /// The plugin responded with an incompatible protocol version.
    #[error("protocol version mismatch for plugin '{name}': expected {expected}, got {got}")]
    ProtocolMismatch {
        /// The plugin name.
        name: String,
        /// The version the host requires.
        expected: u32,
        /// The version the plugin acknowledged.
        got: u32,
    },

    /// A provider kind is already registered by another plugin.
    #[error("duplicate LLM provider kind '{kind}' (already provided by '{existing_plugin}')")]
    DuplicateProvider {
        /// The conflicting provider kind.
        kind: String,
        /// The plugin that already registered this kind.
        existing_plugin: String,
    },

    /// A plugin IPC call failed at the execution level.
    #[error("plugin execution failed: {message}")]
    ExecutionFailed {
        /// Descriptive failure message.
        message: String,
    },

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
    TransportFailed {
        /// Descriptive transport failure message.
        message: String,
    },

    /// The circuit breaker is open after consecutive failures.
    #[error(
        "circuit breaker open for tool '{tool}' after {consecutive_failures} consecutive failures; call paused"
    )]
    CircuitOpen {
        /// The tool whose breaker is open.
        tool: String,
        /// Consecutive failure count that tripped the breaker.
        consecutive_failures: u32,
    },

    /// Error originating from the underlying tool protocol (IPC).
    #[error(transparent)]
    Protocol(#[from] ene_plugin_proto::ToolError),

    /// A tool name collision in [`CompositeToolRegistry`](crate::CompositeToolRegistry).
    ///
    /// Per API v1 / #135, name collision is a hard error at every
    /// registry layer.
    #[error("Duplicate tool name: {tool_name}")]
    DuplicateToolName {
        /// Colliding tool name.
        tool_name: String,
    },

    /// I/O error during plugin spawning or socket management.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

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
    #[error("MCP Invalid tool name: {0}")]
    McpInvalidName(String),
}

impl PluginHostError {
    /// Creates an [`ExecutionFailed`](Self::ExecutionFailed) error.
    pub fn execution(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }

    /// Creates a [`TransportFailed`](Self::TransportFailed) error.
    pub fn transport(message: impl Into<String>) -> Self {
        Self::TransportFailed {
            message: message.into(),
        }
    }

    /// Creates a protocol `IpcClient` error with the given message.
    pub fn ipc(message: impl Into<String>) -> Self {
        Self::Protocol(ene_plugin_proto::ToolError::ipc_client(message))
    }
}

/// Alias for backward compatibility with code that used `ToolHostError`.
pub type ToolHostError = PluginHostError;
