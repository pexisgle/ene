//! Conversation entry author roles.

use serde::{Deserialize, Serialize};

/// Represents the role of a message author in conversation history.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// System instruction or context.
    System,
    /// Human user prompt.
    User,
    /// AI assistant response.
    Assistant,
}
