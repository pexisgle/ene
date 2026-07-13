use thiserror::Error;

/// Error types for the ene AI core subsystem.
#[derive(Error, Debug)]
pub enum EneRuntimeError {
    /// No character card has been loaded.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// LLM provider creation or initialization failed. Callers can
    /// downcast to `LlmProviderError` to dispatch on the underlying cause
    /// (auth, rate limit, network, truncated, content filter, provider).
    #[error(transparent)]
    Provider(#[from] ene_ai::LlmProviderError),
    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    /// Memory store error.
    #[error(transparent)]
    Memory(#[from] ene_store::EneMemoryError),
    /// Session error.
    #[error(transparent)]
    Session(#[from] ene_mind::EneSessionError),
    /// Tool host error.
    #[error(transparent)]
    Tool(#[from] ene_tool_host::EneToolHostError),
    /// Embedding error (init or transport). Callers can downcast to
    /// `EmbeddingError` to dispatch on the variant.
    #[error(transparent)]
    Embedding(#[from] ene_ai::EmbeddingError),
    /// Task channel closed.
    #[error("Task channel closed")]
    ChannelClosed,
    /// A required mind-streaming dependency is unavailable.
    #[error("Mind streaming prerequisite missing: {0}")]
    MindPrerequisite(&'static str),
    /// Cognitive runtime error.
    #[error(transparent)]
    Cognition(#[from] ene_mind::CognitionError),
    /// Bootstrap misconfiguration or internal failure.
    #[error("Bootstrap error: {0}")]
    Bootstrap(String),
}
