use serde::{Deserialize, Serialize};

use crate::TokenUsage;

/// Unified chat message formats for LLM providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "role")]
pub enum LlmMessage {
    /// System instruction message.
    System {
        /// The system prompt content.
        content: String,
    },
    /// User prompt message, supporting multimodal text and image parts.
    User {
        /// The list of content parts (text and/or images).
        parts: Vec<UserMessagePart>,
    },
    /// Assistant reply, potentially including content and/or tool executions.
    Assistant {
        /// The text content of the assistant's reply, if any.
        content: Option<String>,
        /// Tool calls initiated by the model, if any.
        tool_calls: Option<Vec<LlmToolCall>>,
    },
    /// Response from a executed tool.
    Tool {
        /// The ID of the tool call this response is for.
        tool_call_id: String,
        /// The tool execution result content.
        content: String,
    },
}

/// Unified user prompt content part (multimodal support).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserMessagePart {
    /// A block of text.
    Text {
        /// The text content.
        text: String,
    },
    /// A base64-encoded screenshot or image.
    Image {
        /// Base64-encoded image data (with data URI prefix).
        base64_image_data: String,
    },
}

/// Generic representation of a tool call initiated by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmToolCall {
    /// Unique identifier for this specific tool execution request.
    pub id: String,
    /// The name of the tool to invoke.
    pub name: String,
    /// Arguments for the tool in JSON-serialized format.
    pub arguments: String,
}

/// Generic representation of a streaming response chunk.
#[derive(Debug, Clone)]
pub struct LlmResponseChunk {
    /// Text delta generated in this chunk, if any.
    pub text_delta: Option<String>,
    /// Tool call updates generated in this chunk, if any.
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

/// A completed (non-streaming) chat response: the assistant text plus any
/// token usage the provider reported.
///
/// Returned by [`crate::LlmProvider::chat_completion`]. `usage` is `None` when
/// the provider does not report usage, in which case callers that need a count
/// fall back to a character-based estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmCompletion {
    /// The generated assistant text.
    pub text: String,
    /// Token usage reported by the provider, if any.
    pub usage: Option<TokenUsage>,
}

impl LlmCompletion {
    /// A completion with no usage information.
    #[must_use]
    pub fn text_only(text: String) -> Self {
        Self { text, usage: None }
    }
}

impl From<String> for LlmCompletion {
    /// Wrap a bare text response as a completion with no usage.
    fn from(text: String) -> Self {
        Self::text_only(text)
    }
}

/// Generic representation of a tool call fragment in a stream.
#[derive(Debug, Clone)]
pub struct LlmToolCallChunk {
    /// The index of the tool call in the array.
    pub index: usize,
    /// Optional tool call ID.
    pub id: Option<String>,
    /// Optional tool name delta.
    pub name: Option<String>,
    /// Optional tool arguments delta.
    pub arguments: Option<String>,
}
