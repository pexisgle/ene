//! Unified error type for the connector / credential layer.

use thiserror::Error;

/// Errors produced by connector identity and credential operations.
///
/// Connection-lifecycle variants are not part of this enum; process
/// supervision and its failure modes live in `ene-plugin-host`. The credential
/// vault and OAuth flow are expected to extend this with e.g. missing-credential
/// / authorization-required variants when there is a concrete consumer.
#[derive(Debug, Error)]
pub enum ConnectorError {
    /// Authentication was attempted and rejected (bad or revoked secret).
    #[error("authentication failed: {0}")]
    Auth(String),
    /// An OAuth access token has expired and needs refreshing.
    #[error("token expired: {0}")]
    TokenExpired(String),
    /// A transport-level failure while talking to the external service.
    #[error("transport error: {0}")]
    Transport(String),
    /// An operation exceeded its deadline.
    #[error("timeout: {0}")]
    Timeout(String),
    /// A requested connector or credential was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// An underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),
    /// An internal error (invalid input, invariant violation, etc.).
    #[error("internal error: {0}")]
    Internal(String),
}

impl ConnectorError {
    /// Creates an [`Auth`](Self::Auth) error.
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    /// Creates a [`Transport`](Self::Transport) error.
    #[must_use]
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    /// Creates a [`Timeout`](Self::Timeout) error.
    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }

    /// Creates a [`NotFound`](Self::NotFound) error.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Creates an [`Internal`](Self::Internal) error.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl From<std::io::Error> for ConnectorError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
