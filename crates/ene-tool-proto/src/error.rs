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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_error_display_not_found() {
        let err = ToolError::NotFound {
            tool_name: "foo".into(),
        };
        assert_eq!(format!("{err}"), "Tool not found: foo");
    }

    #[test]
    fn tool_error_display_invalid_arguments() {
        let err = ToolError::InvalidArguments {
            message: "bad input".into(),
        };
        assert_eq!(format!("{err}"), "Invalid arguments: bad input");
    }

    #[test]
    fn tool_error_display_execution_failed() {
        let err = ToolError::ExecutionFailed {
            message: "process crashed".into(),
        };
        assert_eq!(format!("{err}"), "Execution failed: process crashed");
    }

    #[test]
    fn tool_error_display_sandbox_violation() {
        let err = ToolError::SandboxViolation {
            message: "path denied".into(),
        };
        assert_eq!(format!("{err}"), "Sandbox violation: path denied");
    }

    #[test]
    fn tool_error_display_permission_denied() {
        let err = ToolError::PermissionDenied {
            message: "not allowed".into(),
        };
        assert_eq!(format!("{err}"), "Permission denied: not allowed");
    }

    #[test]
    fn tool_error_display_io() {
        let err = ToolError::IoError {
            message: "file not found".into(),
        };
        assert_eq!(format!("{err}"), "I/O error: file not found");
    }

    #[test]
    fn tool_error_display_timeout() {
        let err = ToolError::Timeout {
            message: "took too long".into(),
        };
        assert_eq!(format!("{err}"), "Timeout: took too long");
    }

    #[test]
    fn tool_error_display_internal() {
        let err = ToolError::Internal {
            message: "something broke".into(),
        };
        assert_eq!(format!("{err}"), "Internal error: something broke");
    }

    #[test]
    fn tool_error_display_ipc_transport() {
        let err = ToolError::IpcTransport {
            message: "connection lost".into(),
        };
        assert_eq!(format!("{err}"), "IPC transport error: connection lost");
    }

    #[test]
    fn tool_error_is_std_error() {
        use std::error::Error;
        let err = ToolError::NotFound {
            tool_name: "x".into(),
        };
        assert!(err.source().is_none());
    }

    #[test]
    fn tool_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let tool_err: ToolError = io_err.into();
        assert!(matches!(tool_err, ToolError::IoError { .. }));
    }

    #[test]
    fn tool_error_serde_roundtrip() {
        let err = ToolError::NotFound {
            tool_name: "my_tool".into(),
        };
        let json = serde_json::to_string(&err).unwrap();
        let deser: ToolError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, deser);
    }
}
