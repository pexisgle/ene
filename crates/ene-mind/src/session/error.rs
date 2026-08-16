use thiserror::Error;

#[derive(Error, Debug)]
pub enum EneSessionError {
    #[error("Split not needed")]
    SplitNotNeeded,
    #[error("Task channel closed")]
    ChannelClosed,
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error(transparent)]
    MemoryPort(#[from] ene_core::MemoryPortError),
}
