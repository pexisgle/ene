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
/// Carried inside [`ToolError::UserInputRequired`] and surfaced to the
/// UI as a structured question. The prompt always contains one or more
/// [`QuestionItem`]s; the UI is expected to render one input control per item
/// and return a `Vec<MultiAnswer>` with one entry per item in the same order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputPrompt {
    /// The sub-questions presented to the user. Always non-empty when produced
    /// by a tool; tools must validate `len() >= 1` before emitting the prompt.
    pub items: Vec<QuestionItem>,
}

impl UserInputPrompt {
    /// Construct a new prompt, validating that at least one question item
    /// is present.
    ///
    /// # Errors
    /// Returns [`ToolError::InvalidArguments`] when `items` is empty.
    pub fn new(items: Vec<QuestionItem>) -> Result<Self, ToolError> {
        if items.is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "UserInputPrompt requires at least one question item".to_string(),
            });
        }
        Ok(Self { items })
    }
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

/// Discriminator for the generic message-only [`ToolError::Generic`] variant.
///
/// Each kind corresponds to a former standalone variant of `ToolError` that
/// carried only a `message: String` payload. The kinds are serialized in
/// `snake_case` for use in the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Tool execution failed.
    ExecutionFailed,
    /// A sandbox policy was violated.
    SandboxViolation,
    /// Permission was denied for a destructive operation.
    PermissionDenied,
    /// An I/O error occurred.
    IoError,
    /// The tool call timed out.
    Timeout,
    /// An internal error occurred on the **tool side** — the tool binary
    /// encountered an unexpected condition (e.g. a logic bug, an
    /// unrecoverable state) that does not map to any more specific
    /// kind. Prefer this over [`Other`](Self::Other) for tool-side
    /// failures; `Other` is reserved for host-side catch-alls.
    Internal,
    /// An IPC transport error occurred.
    IpcTransport,
    /// An IPC client (host-side) error.
    IpcClient,
    /// Catch-all for **host-side** failures that don't fit any specific
    /// category. Prefer [`Internal`](Self::Internal) for tool-side errors.
    #[deprecated(
        since = "0.1.0",
        note = "Use `ErrorKind::Internal` for tool-side errors; `Other` is kept only for backward-compatible host-side catch-alls."
    )]
    Other,
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::ExecutionFailed => "Execution failed",
            Self::SandboxViolation => "Sandbox violation",
            Self::PermissionDenied => "Permission denied",
            Self::IoError => "I/O error",
            Self::Timeout => "Timeout",
            Self::Internal => "Internal error",
            Self::IpcTransport => "IPC transport error",
            Self::IpcClient => "IPC client error",
            #[expect(
                deprecated,
                reason = "Other kind kept for backward-compatible Display impl"
            )]
            Self::Other => "Other error",
        };
        f.write_str(label)
    }
}

/// Structured tool error type
///
/// Serializable over IPC and used uniformly across tool crates, core, and host.
///
/// The former message-only variants (`ExecutionFailed`, `SandboxViolation`,
/// `PermissionDenied`, `IoError`, `Timeout`, `Internal`, `IpcTransport`,
/// `IpcClient`, `Other`) are consolidated into [`Generic`](Self::Generic)
/// with an [`ErrorKind`] discriminator. Serde wire compatibility is
/// preserved: the serialized JSON still uses the original externally-tagged
/// variant names (e.g. `{"ExecutionFailed":{"message":"…"}}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "ToolErrorWire", from = "ToolErrorWire")]
pub enum ToolError {
    /// The requested tool was not found.
    NotFound {
        /// Name of the tool that was not found.
        tool_name: String,
    },
    /// The tool name supplied by the caller was invalid (empty,
    /// contained illegal characters, or had leading/trailing
    /// dots). Returned by tool-call entry points (e.g. the plugin
    /// dispatch loop) instead of panicking on malformed input.
    InvalidName {
        /// Why the name was rejected.
        reason: String,
    },
    /// Two providers exposed the same public tool name.
    ///
    /// Name collision is a hard error at provider registration
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
    /// A generic message-only error, discriminated by [`ErrorKind`].
    ///
    /// Consolidates the former `ExecutionFailed`, `SandboxViolation`,
    /// `PermissionDenied`, `IoError`, `Timeout`, `Internal`,
    /// `IpcTransport`, `IpcClient`, and `Other` variants.
    Generic {
        /// The error category.
        kind: ErrorKind,
        /// Human-readable error details.
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
}

impl ToolError {
    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`ExecutionFailed`](ErrorKind::ExecutionFailed).
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::ExecutionFailed,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`SandboxViolation`](ErrorKind::SandboxViolation).
    pub fn sandbox_violation(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::SandboxViolation,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`PermissionDenied`](ErrorKind::PermissionDenied).
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::PermissionDenied,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`IoError`](ErrorKind::IoError).
    pub fn io_error(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::IoError,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`Timeout`](ErrorKind::Timeout).
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::Timeout,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`Internal`](ErrorKind::Internal).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::Internal,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`IpcTransport`](ErrorKind::IpcTransport).
    pub fn ipc_transport(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::IpcTransport,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`IpcClient`](ErrorKind::IpcClient).
    pub fn ipc_client(message: impl Into<String>) -> Self {
        Self::Generic {
            kind: ErrorKind::IpcClient,
            message: message.into(),
        }
    }

    /// Creates a [`Generic`](Self::Generic) error with kind
    /// [`Other`](ErrorKind::Other).
    #[deprecated(
        since = "0.1.0",
        note = "Use `ToolError::internal` for tool-side errors; `other` is kept only for backward-compatible host-side catch-alls."
    )]
    pub fn other(message: impl Into<String>) -> Self {
        #[expect(deprecated, reason = "Constructor for the deprecated Other kind")]
        let kind = ErrorKind::Other;
        Self::Generic {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
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
            Self::Generic { kind, message } => {
                write!(f, "{kind}: {message}")
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
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        Self::io_error(e.to_string())
    }
}

// ── Serde wire compatibility ─────────────────────────────────────────────
//
// The public `ToolError` consolidates nine former message-only variants into
// `Generic { kind, message }`. To preserve the externally-tagged JSON wire
// format expected by older tool binaries (e.g. `{"ExecutionFailed":{"message":"…"}}`),
// serialization goes through a private proxy enum that mirrors the historical
// variant layout.

#[derive(Serialize, Deserialize)]
enum ToolErrorWire {
    NotFound {
        tool_name: String,
    },
    InvalidName {
        reason: String,
    },
    DuplicateName {
        tool_name: String,
    },
    InvalidArguments {
        message: String,
    },
    ExecutionFailed {
        message: String,
    },
    SandboxViolation {
        message: String,
    },
    PermissionDenied {
        message: String,
    },
    IoError {
        message: String,
    },
    Timeout {
        message: String,
    },
    Internal {
        message: String,
    },
    IpcTransport {
        message: String,
    },
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    UserInputRequired {
        request_id: String,
        prompt: UserInputPrompt,
    },
    FileNotFound {
        path: String,
    },
    FileTooLarge {
        path: String,
        size: u64,
        limit: u64,
    },
    CommandBlocked {
        command: String,
        reason: String,
    },
    ShellTimeout {
        command: String,
        timeout_ms: u64,
    },
    ShellOutputTooLarge {
        size: u64,
        limit: u64,
    },
    IpcClient {
        message: String,
    },
    Other {
        message: String,
    },
}

impl From<ToolError> for ToolErrorWire {
    fn from(e: ToolError) -> Self {
        match e {
            ToolError::NotFound { tool_name } => Self::NotFound { tool_name },
            ToolError::InvalidName { reason } => Self::InvalidName { reason },
            ToolError::DuplicateName { tool_name } => Self::DuplicateName { tool_name },
            ToolError::InvalidArguments { message } => Self::InvalidArguments { message },
            ToolError::Generic { kind, message } => match kind {
                ErrorKind::ExecutionFailed => Self::ExecutionFailed { message },
                ErrorKind::SandboxViolation => Self::SandboxViolation { message },
                ErrorKind::PermissionDenied => Self::PermissionDenied { message },
                ErrorKind::IoError => Self::IoError { message },
                ErrorKind::Timeout => Self::Timeout { message },
                ErrorKind::Internal => Self::Internal { message },
                ErrorKind::IpcTransport => Self::IpcTransport { message },
                ErrorKind::IpcClient => Self::IpcClient { message },
                #[expect(
                    deprecated,
                    reason = "Wire compat requires mapping the deprecated Other kind"
                )]
                ErrorKind::Other => Self::Other { message },
            },
            ToolError::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => Self::PermissionRequired {
                request_id,
                action,
                target,
                description,
            },
            ToolError::UserInputRequired { request_id, prompt } => {
                Self::UserInputRequired { request_id, prompt }
            }
            ToolError::FileNotFound { path } => Self::FileNotFound { path },
            ToolError::FileTooLarge { path, size, limit } => {
                Self::FileTooLarge { path, size, limit }
            }
            ToolError::CommandBlocked { command, reason } => {
                Self::CommandBlocked { command, reason }
            }
            ToolError::ShellTimeout {
                command,
                timeout_ms,
            } => Self::ShellTimeout {
                command,
                timeout_ms,
            },
            ToolError::ShellOutputTooLarge { size, limit } => {
                Self::ShellOutputTooLarge { size, limit }
            }
        }
    }
}

impl From<ToolErrorWire> for ToolError {
    fn from(w: ToolErrorWire) -> Self {
        match w {
            ToolErrorWire::NotFound { tool_name } => Self::NotFound { tool_name },
            ToolErrorWire::InvalidName { reason } => Self::InvalidName { reason },
            ToolErrorWire::DuplicateName { tool_name } => Self::DuplicateName { tool_name },
            ToolErrorWire::InvalidArguments { message } => Self::InvalidArguments { message },
            ToolErrorWire::ExecutionFailed { message } => Self::execution_failed(message),
            ToolErrorWire::SandboxViolation { message } => Self::sandbox_violation(message),
            ToolErrorWire::PermissionDenied { message } => Self::permission_denied(message),
            ToolErrorWire::IoError { message } => Self::io_error(message),
            ToolErrorWire::Timeout { message } => Self::timeout(message),
            ToolErrorWire::Internal { message } => Self::internal(message),
            ToolErrorWire::IpcTransport { message } => Self::ipc_transport(message),
            ToolErrorWire::IpcClient { message } => Self::ipc_client(message),
            #[expect(
                deprecated,
                reason = "Wire compat requires deserializing the legacy Other variant"
            )]
            ToolErrorWire::Other { message } => Self::other(message),
            ToolErrorWire::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => Self::PermissionRequired {
                request_id,
                action,
                target,
                description,
            },
            ToolErrorWire::UserInputRequired { request_id, prompt } => {
                Self::UserInputRequired { request_id, prompt }
            }
            ToolErrorWire::FileNotFound { path } => Self::FileNotFound { path },
            ToolErrorWire::FileTooLarge { path, size, limit } => {
                Self::FileTooLarge { path, size, limit }
            }
            ToolErrorWire::CommandBlocked { command, reason } => {
                Self::CommandBlocked { command, reason }
            }
            ToolErrorWire::ShellTimeout {
                command,
                timeout_ms,
            } => Self::ShellTimeout {
                command,
                timeout_ms,
            },
            ToolErrorWire::ShellOutputTooLarge { size, limit } => {
                Self::ShellOutputTooLarge { size, limit }
            }
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
        let err = ToolError::execution_failed("process crashed");
        assert_eq!(format!("{err}"), "Execution failed: process crashed");
    }

    #[test]
    fn tool_error_display_sandbox_violation() {
        let err = ToolError::sandbox_violation("path denied");
        assert_eq!(format!("{err}"), "Sandbox violation: path denied");
    }

    #[test]
    fn tool_error_display_permission_denied() {
        let err = ToolError::permission_denied("not allowed");
        assert_eq!(format!("{err}"), "Permission denied: not allowed");
    }

    #[test]
    fn tool_error_display_io() {
        let err = ToolError::io_error("file not found");
        assert_eq!(format!("{err}"), "I/O error: file not found");
    }

    #[test]
    fn tool_error_display_timeout() {
        let err = ToolError::timeout("took too long");
        assert_eq!(format!("{err}"), "Timeout: took too long");
    }

    #[test]
    fn tool_error_display_internal() {
        let err = ToolError::internal("something broke");
        assert_eq!(format!("{err}"), "Internal error: something broke");
    }

    #[test]
    fn tool_error_display_ipc_transport() {
        let err = ToolError::ipc_transport("connection lost");
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
        assert!(matches!(
            tool_err,
            ToolError::Generic {
                kind: ErrorKind::IoError,
                ..
            }
        ));
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
    fn tool_error_serde_roundtrip_all_variants() {
        let cases: Vec<ToolError> = vec![
            ToolError::NotFound {
                tool_name: "t".into(),
            },
            ToolError::InvalidName {
                reason: "bad".into(),
            },
            ToolError::DuplicateName {
                tool_name: "dup".into(),
            },
            ToolError::InvalidArguments {
                message: "arg".into(),
            },
            ToolError::execution_failed("fail"),
            ToolError::sandbox_violation("sv"),
            ToolError::permission_denied("pd"),
            ToolError::io_error("io"),
            ToolError::timeout("to"),
            ToolError::internal("int"),
            ToolError::ipc_transport("ipc"),
            ToolError::PermissionRequired {
                request_id: "r1".into(),
                action: "write".into(),
                target: "/tmp".into(),
                description: "desc".into(),
            },
            ToolError::UserInputRequired {
                request_id: "u1".into(),
                prompt: UserInputPrompt::new(vec![QuestionItem {
                    question: "Q?".into(),
                    options: vec![],
                    allow_free_text: false,
                }])
                .unwrap(),
            },
            ToolError::FileNotFound { path: "/f".into() },
            ToolError::FileTooLarge {
                path: "/f".into(),
                size: 100,
                limit: 50,
            },
            ToolError::CommandBlocked {
                command: "rm".into(),
                reason: "blocked".into(),
            },
            ToolError::ShellTimeout {
                command: "sleep".into(),
                timeout_ms: 1000,
            },
            ToolError::ShellOutputTooLarge {
                size: 999,
                limit: 100,
            },
            ToolError::ipc_client("ipc_client"),
        ];
        for err in cases {
            let json = serde_json::to_string(&err).unwrap();
            let deser: ToolError = serde_json::from_str(&json).unwrap();
            assert_eq!(err, deser, "roundtrip failed for variant");
        }
    }

    #[test]
    fn tool_error_wire_compat_execution_failed() {
        let err = ToolError::execution_failed("boom");
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"ExecutionFailed":{"message":"boom"}}"#);
        let deser: ToolError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, deser);
    }

    #[test]
    fn tool_error_wire_compat_internal() {
        let err = ToolError::internal("oops");
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"Internal":{"message":"oops"}}"#);
    }

    #[test]
    fn tool_error_wire_compat_ipc_client() {
        let err = ToolError::ipc_client("disconnected");
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"IpcClient":{"message":"disconnected"}}"#);
    }

    #[test]
    fn tool_error_wire_compat_deserialize_legacy_other() {
        let json = r#"{"Other":{"message":"legacy"}}"#;
        let err: ToolError = serde_json::from_str(json).unwrap();
        #[expect(
            deprecated,
            reason = "Test verifies the deprecated Other kind round-trips"
        )]
        let expected = ToolError::other("legacy");
        assert_eq!(err, expected);
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
        let p = UserInputPrompt::new(vec![QuestionItem {
            question: "Proceed?".into(),
            options: vec!["Yes".into(), "No".into()],
            allow_free_text: true,
        }])
        .unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let de: UserInputPrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(p, de);
    }

    #[test]
    fn user_input_prompt_new_rejects_empty() {
        let result = UserInputPrompt::new(vec![]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn tool_error_user_input_required_serde_roundtrip() {
        let err = ToolError::UserInputRequired {
            request_id: "req-1".into(),
            prompt: UserInputPrompt::new(vec![QuestionItem {
                question: "Continue?".into(),
                options: vec!["Yes".into(), "No".into()],
                allow_free_text: false,
            }])
            .unwrap(),
        };
        let json = serde_json::to_string(&err).unwrap();
        let de: ToolError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, de);
    }

    #[test]
    fn tool_error_user_input_required_display() {
        let err = ToolError::UserInputRequired {
            request_id: "abc".into(),
            prompt: UserInputPrompt::new(vec![QuestionItem {
                question: "Q?".into(),
                options: vec!["A".into()],
                allow_free_text: true,
            }])
            .unwrap(),
        };
        let s = format!("{err}");
        assert!(s.contains("abc"));
        assert!(s.contains('1'));
    }

    #[test]
    fn user_input_prompt_multi_items_display() {
        let p = UserInputPrompt::new(vec![
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
        ])
        .unwrap();
        let s = format!("{p}");
        assert!(s.contains("1. Pick a color"));
        assert!(s.contains("2. Your name"));
        assert!(s.contains("red"));
        assert!(s.contains("[free text]"));
    }
}
