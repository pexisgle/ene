use serde::{Deserialize, Serialize};

/// 構造化されたツールエラー型
///
/// IPC越しにもシリアライズ可能で、各ツールクレート・core・host間で
/// 統一的に使用される。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolError {
    NotFound { tool_name: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    IoError { message: String },
    Timeout { message: String },
    Internal { message: String },
    IpcTransport { message: String },
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::NotFound { tool_name } => write!(f, "Tool not found: {tool_name}"),
            ToolError::InvalidArguments { message } => write!(f, "Invalid arguments: {message}"),
            ToolError::ExecutionFailed { message } => write!(f, "Execution failed: {message}"),
            ToolError::SandboxViolation { message } => write!(f, "Sandbox violation: {message}"),
            ToolError::PermissionDenied { message } => write!(f, "Permission denied: {message}"),
            ToolError::IoError { message } => write!(f, "I/O error: {message}"),
            ToolError::Timeout { message } => write!(f, "Timeout: {message}"),
            ToolError::Internal { message } => write!(f, "Internal error: {message}"),
            ToolError::IpcTransport { message } => write!(f, "IPC transport error: {message}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        ToolError::IoError {
            message: e.to_string(),
        }
    }
}
