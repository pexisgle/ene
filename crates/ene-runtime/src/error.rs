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
    /// Plugin host error.
    #[error(transparent)]
    Tool(#[from] ene_plugin_host::PluginHostError),
    /// Tool RAG pipeline error.
    #[error(transparent)]
    ToolRag(#[from] ene_rag::ToolRagError),
    /// Task channel closed.
    #[error("Task channel closed")]
    ChannelClosed,
    /// A required mind-streaming dependency is unavailable.
    #[error("Mind streaming prerequisite missing: {0}")]
    MindPrerequisite(&'static str),
    /// Bootstrap misconfiguration or internal failure.
    #[error("Bootstrap error: {0}")]
    Bootstrap(String),
    /// The scheduler needs the memory store, which is disabled.
    #[error("The scheduler requires the memory store (`store.enabled`)")]
    StoreRequired,
    /// The actor rejected admission of a new background task because its
    /// bounded `JoinSet` was already at capacity (Stage 8). Mirrors
    /// `ene_infer::EngineError::Busy` and [`ene_ai::LlmProviderError::Busy`]:
    /// a full queue fails fast rather than blocking or growing without bound.
    #[error("actor task queue is full (capacity {queue_depth}); try again shortly")]
    Busy {
        /// The configured capacity that was exceeded.
        queue_depth: usize,
    },
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
