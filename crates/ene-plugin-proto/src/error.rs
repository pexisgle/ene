use crate::tool_error::ToolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Auth,
    RateLimit,
    Truncated,
    ContentFilter,
}

impl ProviderErrorKind {
    /// Stable marker used inside the legacy message-only IPC error field.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimit => "rate_limit",
            Self::Truncated => "truncated",
            Self::ContentFilter => "content_filter",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("provider error: {0}")]
    Provider(String),

    /// A provider error whose user-facing category must survive IPC.
    #[error("provider error: {message}")]
    ProviderTyped {
        /// Stable category for the host-side typed error mapping.
        kind: ProviderErrorKind,
        message: String,
    },

    #[error("timeout: {0}")]
    Timeout(String),

    /// Unlike [`Transport`](Self::Transport), this variant preserves the
    /// original [`std::io::Error`] as the error source.
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),

    /// Preserves the original [`ToolError`] as the error source rather than
    /// flattening it into a transport error.
    #[error("tool error: {0}")]
    Tool(#[source] ToolError),
}

impl PluginError {
    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::NotSupported(message.into())
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider(message.into())
    }

    pub fn provider_typed(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self::ProviderTyped {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn provider_error_kind(&self) -> Option<ProviderErrorKind> {
        match self {
            Self::ProviderTyped { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }
}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ToolError> for PluginError {
    fn from(e: ToolError) -> Self {
        Self::Tool(e)
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
        use std::error::Error;
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err: PluginError = io_err.into();
        assert!(matches!(err, PluginError::Io(_)));
        assert!(err.to_string().contains("pipe broke"));
        assert!(err.source().is_some(), "source must be preserved");
    }

    #[test]
    fn plugin_error_from_tool_error() {
        use std::error::Error;
        let tool_err = crate::ToolError::ipc_transport("lost connection");
        let err: PluginError = tool_err.into();
        assert!(matches!(err, PluginError::Tool(_)));
        assert!(err.to_string().contains("lost connection"));
        assert!(err.source().is_some(), "source must be preserved");
    }
}
