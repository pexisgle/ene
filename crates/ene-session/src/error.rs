use thiserror::Error;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("Character card not loaded")]
    NoCharacterCard,
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Split not needed")]
    SplitNotNeeded,
    #[error("Task channel closed")]
    ChannelClosed,
    #[error(transparent)]
    Config(#[from] ene_config::ConfigError),
    #[error(transparent)]
    Embedding(#[from] ene_embedding::error::EmbeddingError),
    #[error(transparent)]
    Memory(#[from] ene_memory::MemoryError),
    #[error("OpenAI API error: {0}")]
    OpenAiError(#[from] async_openai::error::OpenAIError),
}
