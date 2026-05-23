use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("Tool execution failed: {0}")]
    ToolExecutionError(String),
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
    #[error("Browser automation error: {0}")]
    BrowserError(String),
    #[error("App/GUI automation error: {0}")]
    AppError(String),
    #[error("Web search error: {0}")]
    WebSearchError(String),
    #[error("OpenAI API error: {0}")]
    OpenAiError(#[from] async_openai::error::OpenAIError),
    #[error(transparent)]
    Config(#[from] ene_config::ConfigError),
    #[error(transparent)]
    Embedding(#[from] ene_embedding::error::EmbeddingError),
    #[error(transparent)]
    Memory(#[from] ene_memory::MemoryError),
    #[error("Other error: {0}")]
    Other(String),
}
