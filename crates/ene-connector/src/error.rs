//! Unified error type for the connector / credential layer.

use thiserror::Error;

/// Errors produced by connector identity and credential operations.
///
/// Connection-lifecycle variants are not part of this enum; process
/// supervision and its failure modes live in `ene-plugin-host`. The credential
/// vault and OAuth flow extend this with the missing-credential /
/// authorization-required variants below (concrete consumer: the host-service
/// `credential` passenger in `ene-plugin-host`).
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
    /// A requested credential does not exist in the vault.
    ///
    /// Carries only non-secret display metadata: the label and help URL guide
    /// a settings UI; the raw value never appears in the error.
    #[error("credential missing: {label}")]
    CredentialMissing {
        /// Credential id (as requested on the wire).
        id: String,
        /// Non-secret display label for setup UI guidance.
        label: String,
        /// Optional URL pointing at setup/help UI.
        help_url: Option<String>,
    },
    /// The requested credential is outside the requesting plugin's declared
    /// scope.
    #[error("credential scope denied: {0}")]
    ScopeDenied(String),
    /// The credential is expired and needs re-authorization.
    #[error("credential refresh required: {0}")]
    RefreshRequired(String),
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

    /// Creates a [`CredentialMissing`](Self::CredentialMissing) error.
    ///
    /// `label` is non-secret display metadata; pass the id itself when no
    /// friendlier label is known.
    #[must_use]
    pub fn credential_missing(
        id: impl Into<String>,
        label: impl Into<String>,
        help_url: Option<String>,
    ) -> Self {
        Self::CredentialMissing {
            id: id.into(),
            label: label.into(),
            help_url,
        }
    }

    /// Creates a [`ScopeDenied`](Self::ScopeDenied) error.
    #[must_use]
    pub fn scope_denied(id: impl Into<String>) -> Self {
        Self::ScopeDenied(id.into())
    }

    /// Creates a [`RefreshRequired`](Self::RefreshRequired) error.
    #[must_use]
    pub fn refresh_required(id: impl Into<String>) -> Self {
        Self::RefreshRequired(id.into())
    }
}

impl From<std::io::Error> for ConnectorError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
