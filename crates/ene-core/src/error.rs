use thiserror::Error;

/// Error types for the ene AI core subsystem.
#[derive(Error, Debug)]
pub enum EneCoreError {
    /// No character card has been loaded.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// LLM provider creation or initialization failed.
    #[error("Provider error: {0}")]
    Provider(String),
    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    /// Memory store error.
    #[error(transparent)]
    Memory(#[from] ene_memory::EneMemoryError),
    /// Session error.
    #[error(transparent)]
    Session(#[from] ene_session::EneSessionError),
    /// Tool host error.
    #[error(transparent)]
    Tool(#[from] ene_tool_host::EneToolHostError),
    /// Embedding error.
    #[error("Embedding error: {0}")]
    EmbeddingError(String),
    /// Task channel closed.
    #[error("Task channel closed")]
    ChannelClosed,
}
