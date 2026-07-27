//! Key-fact domain model (user-specific key/value summarization output).
//!
//! Moved from `ene-store` (#270).

use serde::{Deserialize, Serialize};

/// A key-value fact about the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFact {
    /// The key identifier for this fact.
    pub key: String,
    /// The value associated with the key.
    pub value: String,
}
