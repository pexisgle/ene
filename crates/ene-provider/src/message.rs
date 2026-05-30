use serde::{Deserialize, Serialize};

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
