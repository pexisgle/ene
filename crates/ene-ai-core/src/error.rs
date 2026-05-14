use thiserror::Error;

#[derive(Error, Debug)]
pub enum AiCoreError {
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl { env_var: String },
    #[error("Missing API key: set {env_var} or configure AI API Key")]
    MissingApiKey { env_var: String },
    #[error("Character card not loaded")]
    NoCharacterCard,
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("OpenAI API error: {0}")]
    OpenAiError(#[from] async_openai::error::OpenAIError),
    #[error("Tool execution failed: {0}")]
    ToolExecutionError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Memory store error: {0}")]
    MemoryStoreError(String),
    #[error("Embedding error: {0}")]
    EmbeddingError(String),
    #[error("Sandbox violation: {0}")]
    SandboxViolation(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("File too large: {0} bytes (max: {1} bytes)")]
    FileTooLarge(usize, usize),
    #[error("Command blocked: {0}")]
    CommandBlocked(String),
    #[error("Shell execution timed out after {0} ms")]
    ShellTimeout(u64),
    #[error("Shell output exceeded max size ({0} bytes)")]
    ShellOutputTooLarge(usize),
    #[error("Undo failed: {0}")]
    UndoError(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl From<rusqlite::Error> for AiCoreError {
    fn from(e: rusqlite::Error) -> Self {
        AiCoreError::MemoryStoreError(e.to_string())
    }
}

impl From<regex::Error> for AiCoreError {
    fn from(e: regex::Error) -> Self {
        AiCoreError::Unknown(format!("Regex error: {}", e))
    }
}
