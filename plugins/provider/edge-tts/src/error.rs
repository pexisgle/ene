use ene_plugin::PluginError;

/// Variants stay distinct so the retry policy and the plugin boundary
/// mapping stay explicit.
#[derive(Debug, thiserror::Error)]
pub enum EdgeError {
    #[error("invalid edge-tts configuration: {0}")]
    Config(String),
    /// Transport-level connection failure (DNS, TLS, TCP, HTTP 5xx/408/429).
    #[error("edge-tts connection failed: {0}")]
    Connect(String),
    /// The service rejected the handshake or a request (HTTP 4xx). `403`
    /// signals a stale `Sec-MS-GEC` token and is retryable: the client
    /// re-syncs its clock from the response `Date` header first.
    #[error("edge-tts service rejected the request: HTTP {0}")]
    Rejected(u16),
    #[error("edge-tts request failed: {0}")]
    Send(String),
    #[error("edge-tts protocol error: {0}")]
    Protocol(String),
    #[error("edge-tts synthesis timed out")]
    Timeout,
    #[error("edge-tts returned no audio")]
    NoAudio,
    #[error("edge-tts audio decode failed: {0}")]
    Decode(String),
    #[error("edge-tts audio exceeds the {max} byte cap")]
    TooLarge { max: usize },
}

impl EdgeError {
    #[must_use]
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Connect(_) | Self::Send(_) | Self::Timeout | Self::Rejected(403)
        )
    }
}

impl From<EdgeError> for PluginError {
    fn from(error: EdgeError) -> Self {
        match error {
            EdgeError::Protocol(message) => PluginError::protocol(message),
            EdgeError::Timeout => PluginError::timeout("edge-tts synthesis timed out"),
            other => PluginError::provider(other.to_string()),
        }
    }
}
