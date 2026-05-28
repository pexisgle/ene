use thiserror::Error;

/// Error types for the ene AI core subsystem.
#[derive(Error, Debug)]
pub enum EneCoreError {
    /// Missing base URL for API calls.
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl {
        /// The environment variable name for the base URL.
        env_var: String,
    },
    /// Missing API key for API calls.
    #[error("Missing API key: set {env_var} or configure AI API Key")]
    MissingApiKey {
        /// The environment variable name for the API key.
        env_var: String,
    },
    /// No character card has been loaded.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// Failed to read the character card file.
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    /// Failed to parse JSON.
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    /// A tool execution failed.
    #[error("Tool execution failed: {0}")]
    ToolExecutionError(String),
    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),
    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::ConfigError),
    /// Memory store error.
    #[error(transparent)]
    Memory(#[from] ene_memory::MemoryError),
    /// Session error.
    #[error(transparent)]
    Session(#[from] ene_session::SessionError),
    /// Tool host error.
    #[error(transparent)]
    Tool(#[from] ene_tool_host::ToolError),
    /// Embedding error.
    #[error("Embedding error: {0}")]
    EmbeddingError(String),
    /// Sandbox policy violation.
    #[error("Sandbox violation: {0}")]
    SandboxViolation(String),
    /// Permission was denied.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// File not found.
    #[error("File not found: {0}")]
    FileNotFound(String),
    /// File exceeds maximum size.
    #[error("File too large: {0} bytes (max: {1} bytes)")]
    FileTooLarge(usize, usize),
    /// Shell command blocked.
    #[error("Command blocked: {0}")]
    CommandBlocked(String),
    /// Shell command timed out.
    #[error("Shell execution timed out after {0} ms")]
    ShellTimeout(u64),
    /// Shell output too large.
    #[error("Shell output exceeded max size ({0} bytes)")]
    ShellOutputTooLarge(usize),
    /// Undo operation failed.
    #[error("Undo failed: {0}")]
    UndoError(String),
    /// Browser automation error.
    #[error("Browser automation error: {0}")]
    BrowserError(String),
    /// App automation error.
    #[error("App/GUI automation error: {0}")]
    AppError(String),
    /// Web search error.
    #[error("Web search error: {0}")]
    WebSearchError(String),
    /// Failed to build prompt.
    #[error("Prompt building failed: {0}")]
    PromptBuildError(String),
    /// API request failed.
    #[error("API request failed: {0}")]
    ApiRequestError(String),
    /// Session split not needed.
    #[error("Split not needed")]
    SplitNotNeeded,
    /// Task channel closed.
    #[error("Task channel closed")]
    ChannelClosed,
    /// Regex error.
    #[error("Regex error: {0}")]
    RegexError(#[from] regex::Error),
    /// Unknown error.
    #[error("Unknown error: {0}")]
    Unknown(String),
}
