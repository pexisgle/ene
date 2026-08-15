use serde::{Deserialize, Serialize};

use crate::TokenUsage;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "role")]
pub enum LlmMessage {
    System {
        content: String,
    },
    User {
        parts: Vec<UserMessagePart>,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Option<Vec<LlmToolCall>>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserMessagePart {
    Text { text: String },
    Image { base64_image_data: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    /// JSON-serialized tool arguments.
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct LlmResponseChunk {
    pub text_delta: Option<String>,
    pub tool_calls_delta: Option<Vec<LlmToolCallChunk>>,
    /// Token usage for the whole completion, carried on the **final** chunk
    /// when the provider reports it. Intermediate chunks leave this
    /// `None`; providers that never report usage leave every chunk `None` and
    /// the caller falls back to a character-based estimate.
    pub usage: Option<TokenUsage>,
}

impl From<String> for LlmResponseChunk {
    /// The common case for a [`crate::engine_adapter::llm::StreamingLocalLlmEngine`]-wrapped
    /// model whose `Chunk` is a plain detokenized text piece (e.g. llama.cpp's
    /// `LlamaChatModel`): one text delta, no tool call data, no usage.
    fn from(text_delta: String) -> Self {
        Self {
            text_delta: Some(text_delta),
            tool_calls_delta: None,
            usage: None,
        }
    }
}

/// Returned by [`crate::LlmProvider::chat_completion`]. `usage` is `None` when
/// the provider does not report usage, in which case callers that need a count
/// fall back to a character-based estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmCompletion {
    pub text: String,
    pub usage: Option<TokenUsage>,
}

impl LlmCompletion {
    #[must_use]
    pub fn text_only(text: String) -> Self {
        Self { text, usage: None }
    }
}

impl From<String> for LlmCompletion {
    fn from(text: String) -> Self {
        Self::text_only(text)
    }
}

#[derive(Debug, Clone)]
pub struct LlmToolCallChunk {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}
