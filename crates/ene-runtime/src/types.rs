use serde::{Deserialize, Serialize};

/// Unique identifier for a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(String);

impl RequestId {
    /// Wraps the given string value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrows the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes self and returns the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for RequestId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Unique identifier for a single conversation turn.
///
/// Allocated by [`crate::EneHandle::run`] and carried on every turn-scoped
/// [`crate::EneEvent`]. [`crate::EneHandle::cancel`] only cancels the matching id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(String);

impl TurnId {
    /// Allocates a fresh turn id.
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Borrows the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TurnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TurnId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TurnId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Who initiated a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    /// Normal user-driven turn (`EneHandle::run`).
    #[default]
    User,
    /// Unsolicited companion utterance (no synthetic user message in history).
    Proactive,
    /// A turn started by a persistent schedule.
    Scheduled,
}

/// Error returned by [`crate::EneHandle::run`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RunError {
    /// A turn is already in flight (single-flight).
    #[error("runtime is busy with another turn")]
    Busy,
    /// The actor task is no longer running.
    #[error("actor is no longer running")]
    ActorDead,
}

/// Error returned by [`crate::EneHandle::cancel`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CancelError {
    /// The actor task is no longer running.
    #[error("actor is no longer running")]
    ActorDead,
    /// The supplied turn id does not match the active turn.
    #[error("turn id does not match the active turn")]
    TurnMismatch,
}
