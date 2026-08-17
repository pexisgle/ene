use crate::error::KernelError;
use async_trait::async_trait;
use ene_session::{InnerAspect, ProjectedMessage};

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
    pub finish_reason: String,
    pub model_id: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl Default for ModelGeneration {
    fn default() -> Self {
        Self {
            text: String::new(),
            thinking: None,
            inner: Vec::new(),
            finish_reason: "stop".to_owned(),
            model_id: "stub".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// Deterministic echo model for tests and offline boots.
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
            finish_reason: "stop".to_owned(),
            model_id: "echo".to_owned(),
            input_tokens: u32::try_from(request.messages.len()).unwrap_or(0),
            output_tokens,
        })
    }
}
