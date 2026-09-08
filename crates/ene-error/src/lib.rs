//! Minimal shared error vocabulary for the new implementation.
//!
//! Carries a kind and an opaque message only, so error paths cannot become a
//! channel for secret values.

use std::fmt::{Display, Formatter};
use thiserror::Error;

/// Technical failure kinds shared by owner crates.
///
/// Each variant is the future mapping point for one class of `Err`-side
/// technical failure. Domain `Ok`-side outcomes (such as stale reads or
/// held actions) stay in their owner crates and never map here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Caller-supplied input failed validation; maps malformed requests.
    InvalidInput,
    /// A required item was absent; maps missing-entity lookups.
    NotFound,
    /// State already exists or changed underneath; maps write races.
    Conflict,
    /// A dependency or resource is down; maps transient outages.
    Unavailable,
    /// No other kind fits; maps unexpected internal faults.
    Internal,
}

impl Display for ErrorKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let slug = match self {
            Self::InvalidInput => "invalid-input",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        };
        formatter.write_str(slug)
    }
}

/// Shared error type: a [`ErrorKind`] plus an opaque, secret-free message.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{kind}: {message}")]
pub struct EneError {
    kind: ErrorKind,
    message: String,
}

impl EneError {
    /// Creates an error of `kind` with an opaque human-readable `message`.
    ///
    /// Callers must never put secret values into `message`. The type upholds
    /// the secret non-return premise by carrying only the kind and the opaque
    /// message, with no source chain or payload.
    ///
    /// ```
    /// # use ene_error::{EneError, ErrorKind};
    /// let error = EneError::new(ErrorKind::NotFound, "entry missing");
    /// assert_eq!(error.kind(), ErrorKind::NotFound);
    /// assert_eq!(error.message(), "entry missing");
    /// ```
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the technical failure kind.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the opaque, secret-free message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(test)]
mod tests {
    use super::{EneError, ErrorKind};

    #[test]
    fn display_uses_kebab_kind_prefix() {
        let cases = [
            (ErrorKind::InvalidInput, "invalid-input: bad field"),
            (ErrorKind::NotFound, "not-found: bad field"),
            (ErrorKind::Conflict, "conflict: bad field"),
            (ErrorKind::Unavailable, "unavailable: bad field"),
            (ErrorKind::Internal, "internal: bad field"),
        ];
        for (kind, expected) in cases {
            let error = EneError::new(kind, "bad field");
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn kind_slug_is_lowercase_kebab() {
        let cases = [
            (ErrorKind::InvalidInput, "invalid-input"),
            (ErrorKind::NotFound, "not-found"),
            (ErrorKind::Conflict, "conflict"),
            (ErrorKind::Unavailable, "unavailable"),
            (ErrorKind::Internal, "internal"),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.to_string(), expected);
        }
    }

    #[test]
    fn accessors_return_kind_and_message() {
        let error = EneError::new(ErrorKind::Conflict, "already exists");
        assert_eq!(error.kind(), ErrorKind::Conflict);
        assert_eq!(error.message(), "already exists");
    }

    #[test]
    fn error_is_object_safe() {
        let error = EneError::new(ErrorKind::Unavailable, "downstream down");
        let as_dyn: &dyn std::error::Error = &error;
        assert_eq!(as_dyn.to_string(), "unavailable: downstream down");
        assert!(as_dyn.source().is_none());
    }
}
