use thiserror::Error;

/// Errors produced by connector identity and credential operations.
///
/// Every variant is built from fixed strings, identifiers, and structured
/// fields — never from raw secret material. The [`Display`](std::fmt::Display)
/// output is safe to log and to surface in errors, exports, and events.
#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("token expired: {0}")]
    TokenExpired(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("permission required: {action} on {target}")]
    PermissionRequired {
        /// Deterministic request id the approval flow matches against.
        request_id: String,
        action: String,
        target: String,
        /// Human-readable description shown only in the approval prompt.
        description: String,
    },
    #[error("rate limited")]
    RateLimited {
        /// Suggested wait before retrying, when the service disclosed one.
        retry_after: Option<std::time::Duration>,
    },
    /// A webhook signature or replay check failed.
    #[error("webhook rejected: {0}")]
    WebhookRejected(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ConnectorError {
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    #[must_use]
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }

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

    #[must_use]
    pub fn rate_limited(retry_after: Option<std::time::Duration>) -> Self {
        Self::RateLimited { retry_after }
    }

    #[must_use]
    pub fn webhook_rejected(reason: impl Into<String>) -> Self {
        Self::WebhookRejected(reason.into())
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

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

    /// Scrub any secret-shaped content out of the error's string fields.
    ///
    /// Applied at the registry boundary so connector-supplied error text can
    /// never carry a secret into events, status caches, or the CLI/desktop
    /// surfaces. Structured fields (request ids, identifiers) are preserved.
    #[must_use]
    pub fn scrub(self) -> Self {
        use crate::redaction::scrub_secrets;
        match self {
            Self::Auth(message) => Self::Auth(scrub_secrets(&message)),
            Self::TokenExpired(message) => Self::TokenExpired(scrub_secrets(&message)),
            Self::Transport(message) => Self::Transport(scrub_secrets(&message)),
            Self::Timeout(message) => Self::Timeout(scrub_secrets(&message)),
            Self::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => Self::PermissionRequired {
                request_id,
                action: scrub_secrets(&action),
                target: scrub_secrets(&target),
                description: scrub_secrets(&description),
            },
            Self::RateLimited { retry_after } => Self::RateLimited { retry_after },
            Self::WebhookRejected(reason) => Self::WebhookRejected(scrub_secrets(&reason)),
            Self::NotFound(message) => Self::NotFound(scrub_secrets(&message)),
            Self::Io(error) => Self::Io(std::io::Error::new(
                error.kind(),
                scrub_secrets(&error.to_string()),
            )),
            Self::Internal(message) => Self::Internal(scrub_secrets(&message)),
        }
    }
}

impl From<std::io::Error> for ConnectorError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectorError;

    #[test]
    fn scrub_removes_secrets_from_io_errors() {
        let error = ConnectorError::Io(std::io::Error::other(
            "request failed: api_key=sk-io-secret",
        ));
        let scrubbed = error.scrub();

        assert!(!scrubbed.to_string().contains("sk-io-secret"));
        assert!(scrubbed.to_string().contains("api_key=***"));
    }
}
