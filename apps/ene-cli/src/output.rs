//! Machine-readable output support for non-interactive CLI use.
//!
//! Provides a stable JSON / JSONL contract for CI pipelines, shell scripts,
//! and external orchestrators:
//!
//! - Structured results print to **stdout**; diagnostics and progress stay on
//!   **stderr**.
//! - A common error envelope ([`ErrorBody`]) and a fixed set of [exit codes]
//!   (`EXIT_*`) let callers distinguish success, usage errors, timeouts, and
//!   tool failures.
//! - Streaming turns emit one JSON object per line (JSONL) mirroring the chat
//!   `EneEvent` bus ([`StreamEvent`]).
//!
//! [exit codes]: self#constants

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    /// A single pretty-printed JSON document on stdout.
    Json,
    /// One compact JSON object per line on stdout (streaming).
    Jsonl,
}

impl OutputFormat {
    #[must_use]
    pub const fn is_structured(self) -> bool {
        matches!(self, Self::Json | Self::Jsonl)
    }
}

// Kept stable so external callers can branch on them. `2` matches clap's
// convention for argument/usage errors; `130` matches the REPL's Ctrl-C path.

pub const EXIT_OK: i32 = 0;
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_TOOL_FAILED: i32 = 4;
pub const EXIT_BUSY: i32 = 5;
pub const EXIT_CONFIRMATION_REQUIRED: i32 = 6;
pub const EXIT_INTERRUPTED: i32 = 130;

/// Stable error code strings used in the JSON error envelope.
///
/// The full set is part of the stable machine-readable contract; some
/// classes (e.g. `Timeout`) surface as exit codes rather than through an
/// [`OutputError`], so not every variant is constructed internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Usage,
    Runtime,
    Timeout,
    ToolFailed,
    Busy,
    ConfirmationRequired,
}

impl ErrorCode {
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Usage => EXIT_USAGE,
            Self::Runtime => EXIT_RUNTIME,
            Self::Timeout => EXIT_TIMEOUT,
            Self::ToolFailed => EXIT_TOOL_FAILED,
            Self::Busy => EXIT_BUSY,
            Self::ConfirmationRequired => EXIT_CONFIRMATION_REQUIRED,
        }
    }
}

/// The JSON error envelope printed to stdout on failure in structured mode.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug)]
pub struct OutputError {
    pub body: ErrorBody,
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.body.message)
    }
}

impl std::error::Error for OutputError {}

impl OutputError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            body: ErrorBody {
                code,
                message: message.into(),
            },
        }
    }

    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        self.body.code.exit_code()
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct Wrapper<'a> {
            error: &'a ErrorBody,
        }
        serde_json::to_string_pretty(&Wrapper { error: &self.body })
            .unwrap_or_else(|_| format!("{{\"error\":{{\"message\":{}}}}}", self.body.message))
    }
}

pub fn print_json(value: &impl Serialize) -> Result<(), OutputError> {
    let json = serde_json::to_string_pretty(value).map_err(|e| {
        OutputError::new(
            ErrorCode::Runtime,
            format!("JSON serialization failed: {e}"),
        )
    })?;
    println!("{json}");
    Ok(())
}

pub fn print_jsonl(value: &impl Serialize) -> Result<(), OutputError> {
    let json = serde_json::to_string(value).map_err(|e| {
        OutputError::new(
            ErrorCode::Runtime,
            format!("JSON serialization failed: {e}"),
        )
    })?;
    println!("{json}");
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct PerfCue {
    pub name: String,
    /// Cue kind: `expression`, `motion`, `lookat`, or `cancel`.
    pub kind: String,
    /// How the cue was chosen (e.g. `affect`, `llm`).
    pub source: String,
}

/// A JSONL-serializable mirror of the chat `EneEvent` bus.
///
/// Emitted one-per-line during a non-interactive `run`. The schema is stable
/// and independent of the internal `EneEvent` representation.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// The turn has started streaming.
    TurnStarted { turn: String },
    /// A chunk of generated text.
    TextDelta { turn: String, delta: String },
    /// Presentation cues for the turn.
    Performance { turn: String, cues: Vec<PerfCue> },
    /// A tool call was requested.
    ToolCallStart {
        turn: String,
        name: String,
        /// Parsed JSON arguments (raw object if parsing fails).
        arguments: serde_json::Value,
    },
    /// A tool call completed.
    ToolCallResult {
        turn: String,
        name: String,
        result: String,
    },
    /// A permission gate was auto-denied because no `--yes` was supplied.
    PermissionDenied {
        turn: String,
        action: String,
        target: String,
    },
    /// The turn reached a terminal state (exactly one per run).
    Terminal {
        turn: String,
        /// `done`, `failed`, or `cancelled`.
        reason: String,
        /// Error detail when `reason` is `failed`.
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "unit tests use expect for assertions")]

    use super::*;

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(EXIT_OK, 0);
        assert_eq!(EXIT_RUNTIME, 1);
        assert_eq!(EXIT_USAGE, 2);
        assert_eq!(EXIT_TIMEOUT, 3);
        assert_eq!(EXIT_TOOL_FAILED, 4);
        assert_eq!(EXIT_BUSY, 5);
        assert_eq!(EXIT_CONFIRMATION_REQUIRED, 6);
        assert_eq!(EXIT_INTERRUPTED, 130);
    }

    #[test]
    fn error_code_exit_code_mapping_is_stable() {
        assert_eq!(ErrorCode::Usage.exit_code(), EXIT_USAGE);
        assert_eq!(ErrorCode::Runtime.exit_code(), EXIT_RUNTIME);
        assert_eq!(ErrorCode::Timeout.exit_code(), EXIT_TIMEOUT);
        assert_eq!(ErrorCode::ToolFailed.exit_code(), EXIT_TOOL_FAILED);
        assert_eq!(ErrorCode::Busy.exit_code(), EXIT_BUSY);
        assert_eq!(
            ErrorCode::ConfirmationRequired.exit_code(),
            EXIT_CONFIRMATION_REQUIRED
        );
    }

    #[test]
    fn error_envelope_serializes_with_code_and_message() {
        let err = OutputError::new(ErrorCode::Timeout, "exceeded 5s timeout");
        let value: serde_json::Value = serde_json::from_str(&err.to_json()).expect("valid JSON");
        assert_eq!(value["error"]["code"], "timeout");
        assert_eq!(value["error"]["message"], "exceeded 5s timeout");
    }

    #[test]
    fn stream_events_use_snake_case_type_tag() {
        let event = StreamEvent::TextDelta {
            turn: "t1".to_string(),
            delta: "hi".to_string(),
        };
        let value = serde_json::to_value(&event).expect("serializable");
        assert_eq!(value["type"], "text_delta");
        assert_eq!(value["turn"], "t1");
        assert_eq!(value["delta"], "hi");
    }

    #[test]
    fn terminal_event_omits_absent_message() {
        let event = StreamEvent::Terminal {
            turn: "t1".to_string(),
            reason: "done".to_string(),
            message: None,
        };
        let value = serde_json::to_value(&event).expect("serializable");
        assert_eq!(value["type"], "terminal");
        assert!(value.get("message").is_none());
    }
}
