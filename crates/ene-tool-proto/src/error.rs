use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A single sub-question within a [`UserInputPrompt`]. Each item carries its
/// own set of selectable options and free-text flag, allowing heterogeneous
/// questions (e.g. "yes/no" + "type a name") to be presented in the same
/// dialog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct QuestionItem {
    /// The question text shown to the user.
    pub question: String,
    /// Predefined selectable options. Empty when only free-text is allowed.
    #[serde(default)]
    pub options: Vec<String>,
    /// Whether the user can supply a custom answer that is not in `options`.
    #[serde(default)]
    pub allow_free_text: bool,
}

/// A single answer to one sub-question in a [`UserInputPrompt`].
///
/// Returned as a `Vec<MultiAnswer>` in the same order as the prompt's
/// `items`. Use [`MultiAnswer::Skip`] when the user chose to leave the
/// question blank.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MultiAnswer {
    /// The user picked one of the predefined options.
    Selected {
        /// The exact option string the user selected.
        option: String,
    },
    /// The user provided a free-text answer.
    Answer {
        /// The text the user typed.
        text: String,
    },
    /// The user skipped or left this question blank.
    Skip,
}

/// A prompt requesting interactive user input from a tool.
///
/// Carried inside [`EneToolProtoError::UserInputRequired`] and surfaced to the
/// UI as a structured question. The prompt always contains one or more
/// [`QuestionItem`]s; the UI is expected to render one input control per item
/// and return a `Vec<MultiAnswer>` with one entry per item in the same order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputPrompt {
    /// The sub-questions presented to the user. Always non-empty when produced
    /// by a tool; tools must validate `len() >= 1` before emitting the prompt.
    pub items: Vec<QuestionItem>,
}

impl std::fmt::Display for UserInputPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}. {}", i.saturating_add(1), item.question)?;
            if !item.options.is_empty() {
                write!(f, " (options: {})", item.options.join(", "))?;
            }
            if item.allow_free_text {
                write!(f, " [free text]")?;
            }
        }
        Ok(())
    }
}

/// Structured tool error type
///
/// Serializable over IPC and used uniformly across tool crates, core, and host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EneToolProtoError {
    /// The requested tool was not found.
    NotFound {
        /// Name of the tool that was not found.
        tool_name: String,
    },
    /// The tool name supplied by the caller was invalid (empty,
    /// contained illegal characters, or had leading/trailing
    /// dots). Returned by `HostRegistry::call_tool` and other
    /// IPC entry points instead of panicking on malformed input.
    InvalidName {
        /// Why the name was rejected.
        reason: String,
    },
    /// Two providers exposed the same public tool name.
    ///
    /// Name collision is a hard error at `HostRegistry::add_provider`
    /// time — first-wins silent overwrite is not allowed (#135).
    DuplicateName {
        /// Colliding tool name.
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
        /// The action being requested (e.g. "`filesystem_write`").
        action: String,
        /// The target of the action (e.g. file path).
        target: String,
        /// Human-readable description of what is being requested.
        description: String,
    },
    /// Interactive user input is required to proceed (e.g. an `AskQuestion` tool).
    UserInputRequired {
        /// Unique request identifier.
        request_id: String,
        /// The prompt describing the question, options, and input constraints.
        prompt: UserInputPrompt,
    },
    /// The requested file was not found on disk.
    FileNotFound {
        /// Path that was not found.
        path: String,
    },
    /// A file exceeded the configured size limit.
    FileTooLarge {
        /// Path of the offending file.
        path: String,
        /// Actual size in bytes.
        size: u64,
        /// Maximum allowed size in bytes.
        limit: u64,
    },
    /// A shell command was blocked by sandbox policy.
    CommandBlocked {
        /// The command that was blocked.
        command: String,
        /// Reason for the block.
        reason: String,
    },
    /// A shell command timed out.
    ShellTimeout {
        /// The command that was running.
        command: String,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// Shell output exceeded the maximum size limit.
    ShellOutputTooLarge {
        /// Number of bytes produced.
        size: u64,
        /// Configured limit in bytes.
        limit: u64,
    },

    /// An IPC client (host-side) error.
    IpcClient {
        /// Error details.
        message: String,
    },
    /// Catch-all error variant for host-side failures that don't fit any
    /// specific category.
    Other {
        /// Error details.
        message: String,
    },
}

impl std::fmt::Display for EneToolProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { tool_name } => write!(f, "Tool not found: {tool_name}"),
            Self::InvalidName { reason } => {
                write!(f, "Invalid tool name: {reason}")
            }
            Self::DuplicateName { tool_name } => {
                write!(f, "Duplicate tool name: {tool_name}")
            }
            Self::InvalidArguments { message } => {
                write!(f, "Invalid arguments: {message}")
            }
            Self::ExecutionFailed { message } => {
                write!(f, "Execution failed: {message}")
            }
            Self::SandboxViolation { message } => {
                write!(f, "Sandbox violation: {message}")
            }
            Self::PermissionDenied { message } => {
                write!(f, "Permission denied: {message}")
            }
            Self::IoError { message } => write!(f, "I/O error: {message}"),
            Self::Timeout { message } => write!(f, "Timeout: {message}"),
            Self::Internal { message } => write!(f, "Internal error: {message}"),
            Self::IpcTransport { message } => {
                write!(f, "IPC transport error: {message}")
            }
            Self::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => {
                write!(
                    f,
                    "Permission required [id: {request_id}]: {action} on {target} ({description})"
                )
            }
            Self::UserInputRequired { request_id, prompt } => {
                write!(
                    f,
                    "User input required [id: {}]: {} item(s)",
                    request_id,
                    prompt.items.len(),
                )
            }
            Self::FileNotFound { path } => {
                write!(f, "File not found: {path}")
            }
            Self::FileTooLarge { path, size, limit } => {
                write!(f, "File too large: {path} ({size} bytes, max: {limit})")
            }
            Self::CommandBlocked { command, reason } => {
                write!(f, "Command blocked: {command} ({reason})")
            }
            Self::ShellTimeout {
                command,
                timeout_ms,
            } => {
                write!(
                    f,
                    "Shell execution timed out after {timeout_ms} ms: {command}"
                )
            }
            Self::ShellOutputTooLarge { size, limit } => {
                write!(
                    f,
                    "Shell output exceeded max size ({size} bytes, limit: {limit})"
                )
            }

            Self::IpcClient { message } => {
                write!(f, "IPC client error: {message}")
            }
            Self::Other { message } => {
                write!(f, "Other error: {message}")
            }
        }
    }
}

impl std::error::Error for EneToolProtoError {}

impl From<std::io::Error> for EneToolProtoError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError {
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

    #[test]
    fn user_input_prompt_default_fields() {
        let json = r#"{"items":[{"question":"Pick one"}]}"#;
        let p: UserInputPrompt = serde_json::from_str(json).unwrap();
        assert_eq!(p.items.len(), 1);
        let item = p.items.first().unwrap();
        assert_eq!(item.question, "Pick one");
        assert!(item.options.is_empty());
        assert!(!item.allow_free_text);
    }

    #[test]
    fn user_input_prompt_full() {
        let p = UserInputPrompt {
            items: vec![QuestionItem {
                question: "Proceed?".into(),
                options: vec!["Yes".into(), "No".into()],
                allow_free_text: true,
            }],
        };
        let json = serde_json::to_string(&p).unwrap();
        let de: UserInputPrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(p, de);
    }

    #[test]
    fn tool_error_user_input_required_serde_roundtrip() {
        let err = ToolError::UserInputRequired {
            request_id: "req-1".into(),
            prompt: UserInputPrompt {
                items: vec![QuestionItem {
                    question: "Continue?".into(),
                    options: vec!["Yes".into(), "No".into()],
                    allow_free_text: false,
                }],
            },
        };
        let json = serde_json::to_string(&err).unwrap();
        let de: ToolError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, de);
    }

    #[test]
    fn tool_error_user_input_required_display() {
        let err = ToolError::UserInputRequired {
            request_id: "abc".into(),
            prompt: UserInputPrompt {
                items: vec![QuestionItem {
                    question: "Q?".into(),
                    options: vec!["A".into()],
                    allow_free_text: true,
                }],
            },
        };
        let s = format!("{err}");
        assert!(s.contains("abc"));
        assert!(s.contains('1'));
    }

    #[test]
    fn user_input_prompt_multi_items_display() {
        let p = UserInputPrompt {
            items: vec![
                QuestionItem {
                    question: "Pick a color".into(),
                    options: vec!["red".into(), "blue".into()],
                    allow_free_text: false,
                },
                QuestionItem {
                    question: "Your name".into(),
                    options: vec![],
                    allow_free_text: true,
                },
            ],
        };
        let s = format!("{p}");
        assert!(s.contains("1. Pick a color"));
        assert!(s.contains("2. Your name"));
        assert!(s.contains("red"));
        assert!(s.contains("[free text]"));
    }
}
