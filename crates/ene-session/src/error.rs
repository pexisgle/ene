use thiserror::Error;

/// Error types for the session subsystem.
#[derive(Error, Debug)]
pub enum EneSessionError {
    /// No character card has been loaded.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// Failed to read the character card file.
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    /// Failed to parse JSON.
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    /// Session split is not required.
    #[error("Split not needed")]
    SplitNotNeeded,
    /// Task channel closed unexpectedly.
    #[error("Task channel closed")]
    ChannelClosed,
    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),
    /// Embedding error.
    #[error("Embedding error: {0}")]
    Embedding(String),
    /// Memory store error.
    #[error(transparent)]
    Memory(#[from] ene_memory::EneMemoryError),
}
