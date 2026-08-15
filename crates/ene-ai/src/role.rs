use serde::{Deserialize, Serialize};

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
