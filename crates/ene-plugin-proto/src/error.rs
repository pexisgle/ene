//! Plugin error types.

/// Errors that can occur during plugin IPC communication and provider
/// operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// The requested capability or operation is not supported by this plugin.
    #[error("not supported: {0}")]
    NotSupported(String),

    /// An I/O or transport-level error occurred on the IPC connection.
    #[error("transport error: {0}")]
    Transport(String),

    /// A protocol-level error (version mismatch, malformed message, etc.).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A provider-level error (LLM/TTS/STT call failed).
    #[error("provider error: {0}")]
    Provider(String),

    /// An operation timed out.
    #[error("timeout: {0}")]
    Timeout(String),
}

impl PluginError {
    /// Creates a [`NotSupported`](Self::NotSupported) error.
    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::NotSupported(message.into())
    }

    /// Creates a [`Transport`](Self::Transport) error.
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    /// Creates a [`Protocol`](Self::Protocol) error.
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    /// Creates a [`Provider`](Self::Provider) error.
    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider(message.into())
    }

    /// Creates a [`Timeout`](Self::Timeout) error.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }
}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        Self::Transport(e.to_string())
    }
}

impl From<crate::tool_error::ToolError> for PluginError {
    fn from(e: crate::tool_error::ToolError) -> Self {
        Self::Transport(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_error_display_not_supported() {
        let err = PluginError::not_supported("streaming");
        assert_eq!(format!("{err}"), "not supported: streaming");
    }

    #[test]
    fn plugin_error_display_transport() {
        let err = PluginError::transport("connection reset");
        assert_eq!(format!("{err}"), "transport error: connection reset");
    }

    #[test]
    fn plugin_error_display_protocol() {
        let err = PluginError::protocol("version mismatch");
        assert_eq!(format!("{err}"), "protocol error: version mismatch");
    }

    #[test]
    fn plugin_error_display_provider() {
        let err = PluginError::provider("rate limited");
        assert_eq!(format!("{err}"), "provider error: rate limited");
    }

    #[test]
    fn plugin_error_display_timeout() {
        let err = PluginError::timeout("30s exceeded");
        assert_eq!(format!("{err}"), "timeout: 30s exceeded");
    }

    #[test]
    fn plugin_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err: PluginError = io_err.into();
        assert!(matches!(err, PluginError::Transport(_)));
        assert!(err.to_string().contains("pipe broke"));
    }

    #[test]
    fn plugin_error_from_tool_error() {
        let tool_err = crate::ToolError::ipc_transport("lost connection");
        let err: PluginError = tool_err.into();
        assert!(matches!(err, PluginError::Transport(_)));
        assert!(err.to_string().contains("lost connection"));
    }
}
