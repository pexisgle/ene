use ene_config::serde::{Deserialize, Serialize};

/// Structured tool error type
///
/// Serializable over IPC and used uniformly across tool crates, core, and host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(crate = "ene_config::serde")]
pub enum EneToolProtoError {
    /// The requested tool was not found.
    NotFound {
        /// Name of the tool that was not found.
        tool_name: String,
    },
    /// Invalid arguments were passed to a tool call.
    InvalidArguments {
        /// Description of what was invalid.
        message: String,
    },
    /// Tool execution failed.
    ExecutionFailed {
        /// Error details.
        message: String,
    },
    /// A sandbox policy was violated.
    SandboxViolation {
        /// Description of the violation.
        message: String,
    },
    /// Permission was denied for a destructive operation.
    PermissionDenied {
        /// Explanation of why permission was denied.
        message: String,
    },
    /// An I/O error occurred.
    IoError {
        /// I/O error details.
        message: String,
    },
    /// The tool call timed out.
    Timeout {
        /// Timeout details.
        message: String,
    },
    /// An internal error occurred.
    Internal {
        /// Internal error details.
        message: String,
    },
    /// An IPC transport error occurred.
    IpcTransport {
        /// Transport error details.
        message: String,
    },
    /// User permission is required to proceed.
    PermissionRequired {
        /// Unique request identifier.
        request_id: String,
        /// The action being requested (e.g. "filesystem_write").
        action: String,
        /// The target of the action (e.g. file path).
        target: String,
        /// Human-readable description of what is being requested.
        description: String,
    },
}

impl std::fmt::Display for EneToolProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EneToolProtoError::NotFound { tool_name } => write!(f, "Tool not found: {tool_name}"),
            EneToolProtoError::InvalidArguments { message } => {
                write!(f, "Invalid arguments: {message}")
            }
            EneToolProtoError::ExecutionFailed { message } => {
                write!(f, "Execution failed: {message}")
            }
            EneToolProtoError::SandboxViolation { message } => {
                write!(f, "Sandbox violation: {message}")
            }
            EneToolProtoError::PermissionDenied { message } => {
                write!(f, "Permission denied: {message}")
            }
            EneToolProtoError::IoError { message } => write!(f, "I/O error: {message}"),
            EneToolProtoError::Timeout { message } => write!(f, "Timeout: {message}"),
            EneToolProtoError::Internal { message } => write!(f, "Internal error: {message}"),
            EneToolProtoError::IpcTransport { message } => {
                write!(f, "IPC transport error: {message}")
            }
            EneToolProtoError::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => {
                write!(
                    f,
                    "Permission required [id: {}]: {} on {} ({})",
                    request_id, action, target, description
                )
            }
        }
    }
}

impl std::error::Error for EneToolProtoError {}

impl From<std::io::Error> for EneToolProtoError {
    fn from(e: std::io::Error) -> Self {
        EneToolProtoError::IoError {
            message: e.to_string(),
        }
    }
}

/// Type alias for backward compatibility and internal tool module usages.
pub type ToolError = EneToolProtoError;

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
