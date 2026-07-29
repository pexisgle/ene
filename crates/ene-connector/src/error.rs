use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("not connected: {0}")]
    NotConnected(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("token expired: {0}")]
    TokenExpired(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("rate limited: retry after {0:?}")]
    RateLimited(std::time::Duration),
    #[error("policy error: {0}")]
    Policy(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("connector degraded: {0}")]
    Degraded(String),
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ConnectorError {
    pub fn not_connected(message: impl Into<String>) -> Self {
        Self::NotConnected(message.into())
    }
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl From<std::io::Error> for ConnectorError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
