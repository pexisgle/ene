//! Unified error type for the connector / credential layer.

use thiserror::Error;

/// Errors produced by connector identity and credential operations.
///
/// Every variant is built from fixed strings, identifiers, and structured
/// fields — never from raw secret material. The [`Display`](std::fmt::Display)
/// output is safe to log and to surface in errors, exports, and events.
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
    /// The user must approve the action before it may proceed.
    #[error("permission required: {action} on {target}")]
    PermissionRequired {
        /// Deterministic request id the approval flow matches against.
        request_id: String,
        /// Action label (e.g. `connector.connect`).
        action: String,
        /// Target resource (e.g. `connector:github`).
        target: String,
        /// Human-readable description shown only in the approval prompt.
        description: String,
    },
    /// The remote service rejected the request because a rate limit was hit.
    #[error("rate limited")]
    RateLimited {
        /// Suggested wait before retrying, when the service disclosed one.
        retry_after: Option<std::time::Duration>,
    },
    /// A webhook signature or replay check failed.
    #[error("webhook rejected: {0}")]
    WebhookRejected(String),
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

    /// Creates a [`PermissionRequired`](Self::PermissionRequired) error.
    #[must_use]
    pub fn permission_required(
        request_id: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::PermissionRequired {
            request_id: request_id.into(),
            action: action.into(),
            target: target.into(),
            description: description.into(),
        }
    }

    /// Creates a [`RateLimited`](Self::RateLimited) error.
    #[must_use]
    pub fn rate_limited(retry_after: Option<std::time::Duration>) -> Self {
        Self::RateLimited { retry_after }
    }

    /// Creates a [`WebhookRejected`](Self::WebhookRejected) error.
    #[must_use]
    pub fn webhook_rejected(reason: impl Into<String>) -> Self {
        Self::WebhookRejected(reason.into())
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

    /// Returns `true` when a retry after backoff may help.
    ///
    /// Transport and rate-limit failures are transient by nature. Auth,
    /// permission, timeout, and not-found failures are not: retrying them
    /// either cannot succeed or stacks load inside the operation boundary.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::Io(_) | Self::RateLimited { .. }
        )
    }
}

impl From<std::io::Error> for ConnectorError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
