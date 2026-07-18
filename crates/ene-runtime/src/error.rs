use thiserror::Error;

/// Error types for the ene AI core subsystem.
#[derive(Error, Debug)]
pub enum EneRuntimeError {
    /// No character card has been loaded.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// LLM or embedding provider failure.
    #[error(transparent)]
    Ai(#[from] ene_ai::AiError),
    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    /// Memory store error.
    #[error(transparent)]
    Memory(#[from] ene_store::EneMemoryError),
    /// Mind (cognition / session) error.
    #[error(transparent)]
    Mind(#[from] ene_mind::MindError),
    /// Tool host error.
    #[error(transparent)]
    Tool(#[from] ene_tool_host::EneToolHostError),
    /// Task channel closed.
    #[error("Task channel closed")]
    ChannelClosed,
    /// A required mind-streaming dependency is unavailable.
    #[error("Mind streaming prerequisite missing: {0}")]
    MindPrerequisite(&'static str),
    /// Bootstrap misconfiguration or internal failure.
    #[error("Bootstrap error: {0}")]
    Bootstrap(String),
}

impl From<ene_tool_rag::ToolRagError> for EneRuntimeError {
    fn from(value: ene_tool_rag::ToolRagError) -> Self {
        Self::Bootstrap(value.to_string())
    }
}

impl From<ene_ai::LlmProviderError> for EneRuntimeError {
    fn from(value: ene_ai::LlmProviderError) -> Self {
        Self::Ai(value.into())
    }
}

impl From<ene_ai::EmbeddingError> for EneRuntimeError {
    fn from(value: ene_ai::EmbeddingError) -> Self {
        Self::Ai(value.into())
    }
}

impl From<ene_mind::EneSessionError> for EneRuntimeError {
    fn from(value: ene_mind::EneSessionError) -> Self {
        Self::Mind(value.into())
    }
}

impl From<ene_mind::CognitionError> for EneRuntimeError {
    fn from(value: ene_mind::CognitionError) -> Self {
        Self::Mind(value.into())
    }
}
