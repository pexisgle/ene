use thiserror::Error;

/// Error types for the session subsystem.
#[derive(Error, Debug)]
pub enum EneSessionError {
    /// Session split is not required.
    #[error("Split not needed")]
    SplitNotNeeded,
    /// Task channel closed unexpectedly.
    #[error("Task channel closed")]
    ChannelClosed,
    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    /// Embedding error.
    #[error("Embedding error: {0}")]
    Embedding(String),
    /// Memory port error.
    #[error(transparent)]
    MemoryPort(#[from] ene_core::MemoryPortError),
}
