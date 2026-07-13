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
    /// Execution failed (e.g. tool binary not found, MCP client failed to start).
    #[error("Execution failed: {message}")]
    ExecutionFailed {
        /// A descriptive message about the failure.
        message: String,
    },
    /// Two registries exposed the same public tool name.
    ///
    /// Name collision is a hard error at composite build / `add_registry`
    /// time — first-wins silent overwrite is not allowed (#135).
    #[error("Duplicate tool name: {name}")]
    DuplicateToolName {
        /// Colliding tool name.
        name: String,
    },
}

impl EneToolHostError {
    /// Creates a protocol IpcClient error with the given message.
    pub fn ipc(message: impl Into<String>) -> Self {
        Self::Protocol(ene_tool_proto::ToolError::IpcClient {
            message: message.into(),
        })
    }
}

/// Alias for tool host error.
pub type ToolHostError = EneToolHostError;
