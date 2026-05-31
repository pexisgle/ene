use thiserror::Error;

/// Error types for the tool host subsystem.
#[derive(Error, Debug)]
pub enum EneToolHostError {
    /// A tool execution failed.
    #[error("Tool execution failed: {0}")]
    ToolExecutionError(String),
    /// A sandbox policy violation occurred.
    #[error("Sandbox violation: {0}")]
    SandboxViolation(String),
    /// Permission was denied for an operation.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// Permission must be granted before proceeding.
    #[error("Permission required: {action} on {target} ({description})")]
    PermissionRequired {
        /// Unique identifier for the permission request.
        request_id: String,
        /// The action requesting permission.
        action: String,
        /// The target of the action.
        target: String,
        /// Human-readable description.
        description: String,
    },
    /// The requested file was not found.
    #[error("File not found: {0}")]
    FileNotFound(String),
    /// The file exceeds the maximum allowed size.
    #[error("File too large: {0} bytes (max: {1} bytes)")]
    FileTooLarge(usize, usize),
    /// The shell command was blocked by sandbox policy.
    #[error("Command blocked: {0}")]
    CommandBlocked(String),
    /// A shell command timed out.
    #[error("Shell execution timed out after {0} ms")]
    ShellTimeout(u64),
    /// Shell output exceeded the maximum size limit.
    #[error("Shell output exceeded max size ({0} bytes)")]
    ShellOutputTooLarge(usize),
    /// A browser automation error occurred.
    #[error("Browser automation error: {0}")]
    BrowserError(String),
    /// An app automation error occurred.
    #[error("App/GUI automation error: {0}")]
    AppError(String),
    /// A web search error occurred.
    #[error("Web search error: {0}")]
    WebSearchError(String),
    /// IPC client connection or transport error.
    #[error("IPC client error: {0}")]
    IpcClient(String),
    /// OpenAI API error.
    #[error("OpenAI API error: {0}")]
    OpenAiError(#[from] async_openai::error::OpenAIError),
    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::ConfigError),
    /// Embedding error.
    #[error("Embedding error: {0}")]
    Embedding(String),

    /// Memory store error.
    #[error(transparent)]
    Memory(#[from] ene_memory::MemoryError),
    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}

/// Type alias for internal module usages.
pub type ToolError = EneToolHostError;
