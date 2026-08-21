use crate::error::KernelError;
use async_trait::async_trait;
use ene_session::{InnerAspect, ProjectedMessage, Role};
use serde_json::Value;

/// Conversation LLM used by the dialogue lane.
#[async_trait]
pub trait ConversationModel: Send + Sync {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError>;
}

/// Model-visible request. Content must be reconstructable from the session log (L-1).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub messages: Vec<ProjectedMessage>,
}

/// One generation step.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelGeneration {
    pub text: String,
    pub thinking: Option<String>,
    pub inner: Vec<(InnerAspect, String)>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub model_id: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Model-requested tool invocation. Empty on echo / speech-only generations.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

impl Default for ModelGeneration {
    fn default() -> Self {
        Self {
            text: String::new(),
            thinking: None,
            inner: Vec::new(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            model_id: "stub".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// Deterministic echo model for tests and offline boots. Not an acceptance path:
/// it never emits `tool_calls`.
pub struct EchoModel;

#[async_trait]
impl ConversationModel for EchoModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        let last = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ene_session::Role::User)
            .map_or_else(|| "hello".to_owned(), ProjectedMessage::text);
        let interrupted = request
            .messages
            .iter()
            .any(|message| message.text().contains("interrupted"));
        let text = if interrupted {
            format!("that last turn was interrupted; you said: {last}")
        } else {
            format!("ack: {last}")
        };
        let output_tokens = u32::try_from(text.len()).unwrap_or(u32::MAX);
        Ok(ModelGeneration {
            text,
            thinking: None,
            inner: vec![(InnerAspect::Thought, format!("noted: {last}"))],
            tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            model_id: "echo".to_owned(),
            input_tokens: u32::try_from(request.messages.len()).unwrap_or(0),
            output_tokens,
        })
    }
}

/// Test double that emits one `utility.calc` call, then speaks the tool result.
///
/// Use this for lane / HTTP acceptance of tool calling. Echo never takes that path.
pub struct ToolCallingModel;

#[async_trait]
impl ConversationModel for ToolCallingModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        if let Some(result) = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == Role::Tool)
        {
            let text = format!("result: {}", result.text());
            let output_tokens = u32::try_from(text.len()).unwrap_or(u32::MAX);
            return Ok(ModelGeneration {
                text,
                finish_reason: "stop".to_owned(),
                model_id: "tool-calling".to_owned(),
                input_tokens: u32::try_from(request.messages.len()).unwrap_or(0),
                output_tokens,
                ..ModelGeneration::default()
            });
        }
        let last = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == Role::User)
            .map_or_else(String::new, ProjectedMessage::text);
        let lower = last.to_ascii_lowercase();
        if lower.contains("calc") || lower.contains("1+2") {
            return Ok(ModelGeneration {
                tool_calls: vec![ToolCall {
                    name: "utility.calc".to_owned(),
                    arguments: serde_json::json!({"expr": "1+2*3"}),
                }],
                finish_reason: "tool_calls".to_owned(),
                model_id: "tool-calling".to_owned(),
                input_tokens: u32::try_from(request.messages.len()).unwrap_or(0),
                ..ModelGeneration::default()
            });
        }
        EchoModel.generate(request).await
    }
}
