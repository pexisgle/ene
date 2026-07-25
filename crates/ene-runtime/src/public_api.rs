//! Stable public API v1 surface for external clients (#189).
//!
//! The actor facade remains [`crate::EneHandle`]. This module defines the
//! versioned JSON/serde mirror of chat events, redaction helpers, and the
//! `API_VERSION` constant. Internal modules such as `streaming` and
//! `message_builder` are not part of this contract.

use crate::handle::{EneEvent, EneStatus, TerminalReason};
use crate::types::TurnOrigin;
use serde::{Deserialize, Serialize};

/// Public API major version string for ene-runtime host contracts.
///
/// Bump only for intentional wire/semantic breaks. Additive enum variants and
/// optional fields do not require a bump when clients ignore unknown keys.
pub const API_VERSION: &str = "1";

/// A single presentation cue in a [`PublicChatEvent::Performance`] event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicPerfCue {
    /// Cue name (expression / motion label).
    pub name: String,
    /// Cue kind: `expression`, `motion`, `lookat`, or `cancel`.
    pub kind: String,
    /// How the cue was chosen (e.g. `affect`, `llm_command`).
    pub source: String,
}

/// Stable JSON mirror of the chat [`EneEvent`] bus (#189).
///
/// Tagged with `type` in `snake_case`, aligned with the CLI JSONL schema and
/// extended with fields the host contract documents (`origin`, background
/// tool completion, gates). Prefer this type over serializing [`EneEvent`]
/// directly when exposing events outside the process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublicChatEvent {
    /// The turn has started streaming.
    TurnStarted {
        /// Turn id (UUID string).
        turn: String,
        /// Who initiated the turn.
        origin: String,
    },
    /// A chunk of generated text.
    TextDelta {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Text chunk (markers stripped).
        delta: String,
    },
    /// Presentation cues for the turn.
    Performance {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Cue list.
        cues: Vec<PublicPerfCue>,
        /// Aggregate cue source for the batch.
        source: String,
    },
    /// A tool call was requested.
    ToolCallStart {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Tool name.
        name: String,
        /// Parsed JSON arguments (object), after redaction when converted via
        /// [`PublicChatEvent::from_ene_event`].
        arguments: serde_json::Value,
    },
    /// A tool call completed.
    ToolCallResult {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Tool name.
        name: String,
        /// Tool output (may be truncated / redacted).
        result: String,
    },
    /// A deferred background tool task reached a terminal state.
    ToolBackgroundCompleted {
        /// Tool name.
        tool_name: String,
        /// Background task id.
        task_id: String,
        /// Terminal status string (`completed`, `failed`, `cancelled`, …).
        status: String,
    },
    /// A destructive operation requires approval.
    PermissionRequired {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Permission request id.
        request_id: String,
        /// Operation category.
        action: String,
        /// Target resource.
        target: String,
        /// Human-readable description.
        description: String,
    },
    /// An interactive tool needs user input.
    UserInputRequired {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Input request id.
        request_id: String,
        /// Prompt kind label from the tool ABI.
        prompt_kind: String,
    },
    /// Rolling context compression completed for this turn.
    ContextCompressed {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Compression level label.
        level: String,
    },
    /// The turn reached a terminal state (exactly one per run).
    Terminal {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// `done`, `failed`, or `cancelled`.
        reason: String,
        /// Error detail when `reason` is `failed`.
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Actor status changed.
    StatusChanged {
        /// `idle`, `running`, or `error`.
        status: String,
    },
    /// A chunk of synthesized PCM audio from the TTS pipeline.
    AudioChunk {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Number of PCM samples in this chunk.
        pcm_len: usize,
        /// Sample rate in Hz.
        sample_rate: u32,
        /// Whether this is the final audio chunk for the turn.
        is_final: bool,
    },
}

impl PublicChatEvent {
    /// Convert an internal chat event into the stable public mirror with
    /// redaction applied to tool arguments and sensitive text.
    #[must_use]
    pub fn from_ene_event(event: &EneEvent) -> Self {
        match event {
            EneEvent::TurnStarted { turn, origin } => Self::TurnStarted {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
            },
            EneEvent::TextDelta {
                turn,
                origin,
                delta,
            } => Self::TextDelta {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                delta: redact_text(delta),
            },
            EneEvent::Performance {
                turn,
                origin,
                cues,
                source,
            } => Self::Performance {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                cues: cues
                    .iter()
                    .map(|c| PublicPerfCue {
                        name: c.name.clone(),
                        kind: perf_kind_label(c.kind).to_string(),
                        source: cue_source_label(*source),
                    })
                    .collect(),
                source: cue_source_label(*source),
            },
            EneEvent::ToolCallStart {
                turn,
                origin,
                name,
                arguments,
            } => Self::ToolCallStart {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                name: name.clone(),
                arguments: redact_tool_arguments_json(arguments),
            },
            EneEvent::ToolCallResult {
                turn,
                origin,
                name,
                result,
            } => Self::ToolCallResult {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                name: name.clone(),
                result: redact_text(result),
            },
            EneEvent::ToolBackgroundCompleted {
                tool_name,
                task_id,
                status,
            } => Self::ToolBackgroundCompleted {
                tool_name: tool_name.clone(),
                task_id: task_id.clone(),
                status: format!("{status:?}").to_ascii_lowercase(),
            },
            EneEvent::PermissionRequired {
                turn,
                origin,
                request_id,
                action,
                target,
                description,
            } => Self::PermissionRequired {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                request_id: request_id.to_string(),
                action: action.clone(),
                target: target.clone(),
                description: description.clone(),
            },
            EneEvent::UserInputRequired {
                turn,
                origin,
                request_id,
                prompt: _,
            } => Self::UserInputRequired {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                request_id: request_id.to_string(),
                prompt_kind: "user_input".to_string(),
            },
            EneEvent::ContextCompressed {
                turn,
                origin,
                level,
            } => Self::ContextCompressed {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                level: level.clone(),
            },
            EneEvent::Terminal {
                turn,
                origin,
                reason,
            } => {
                let (reason_label, message) = match reason {
                    TerminalReason::Done => ("done".to_string(), None),
                    TerminalReason::Failed { message } => {
                        ("failed".to_string(), Some(redact_text(message)))
                    }
                    TerminalReason::Cancelled => ("cancelled".to_string(), None),
                };
                Self::Terminal {
                    turn: turn.to_string(),
                    origin: origin_label(*origin).to_string(),
                    reason: reason_label,
                    message,
                }
            }
            EneEvent::StatusChanged { status } => Self::StatusChanged {
                status: match status {
                    EneStatus::Idle => "idle".to_string(),
                    EneStatus::Running => "running".to_string(),
                    EneStatus::Error => "error".to_string(),
                },
            },
            EneEvent::AudioChunk {
                turn,
                origin,
                pcm,
                sample_rate,
                is_final,
            } => Self::AudioChunk {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                pcm_len: pcm.len(),
                sample_rate: *sample_rate,
                is_final: *is_final,
            },
            EneEvent::PendingCandidateAvailable { count } => Self::StatusChanged {
                status: format!("pending_candidates_{count}"),
            },
        }
    }
}

const fn origin_label(origin: TurnOrigin) -> &'static str {
    match origin {
        TurnOrigin::User => "user",
        TurnOrigin::Proactive => "proactive",
    }
}

fn perf_kind_label(kind: ene_mind::PerfKind) -> &'static str {
    match kind {
        ene_mind::PerfKind::Expression => "expression",
        ene_mind::PerfKind::Motion => "motion",
        ene_mind::PerfKind::LookAt => "lookat",
        ene_mind::PerfKind::Cancel => "cancel",
    }
}

fn cue_source_label(source: ene_mind::CueSource) -> String {
    source.as_str().to_string()
}

/// Redact obvious secrets from free-form text (API keys, bearer tokens, PEM).
pub fn redact_text(input: &str) -> String {
    ene_store::redact_secrets(input)
}

/// Redact sensitive keys inside a tool-argument JSON string.
pub fn redact_tool_arguments(arguments: &str) -> String {
    ene_store::redact_arguments(arguments)
}

/// Parse tool arguments as JSON (object preferred) after redaction.
pub fn redact_tool_arguments_json(arguments: &str) -> serde_json::Value {
    let redacted = redact_tool_arguments(arguments);
    serde_json::from_str(&redacted).unwrap_or_else(|_| serde_json::json!({ "raw": redacted }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TurnId;

    #[test]
    fn api_version_is_one() {
        assert_eq!(API_VERSION, "1");
    }

    #[test]
    fn public_chat_event_uses_snake_case_type_tag() {
        let event = PublicChatEvent::TextDelta {
            turn: "t1".into(),
            origin: "user".into(),
            delta: "hi".into(),
        };
        let value = serde_json::to_value(&event).expect("serializable");
        assert_eq!(value["type"], "text_delta");
        assert_eq!(value["origin"], "user");
    }

    #[test]
    fn from_ene_event_redacts_tool_arguments() {
        let event = EneEvent::ToolCallStart {
            turn: TurnId::new(),
            origin: TurnOrigin::User,
            name: "fs.write".into(),
            arguments: r#"{"path":"/tmp/x","api_key":"sk-secret-value-here"}"#.into(),
        };
        let public = PublicChatEvent::from_ene_event(&event);
        let PublicChatEvent::ToolCallStart { arguments, .. } = public else {
            panic!("expected ToolCallStart");
        };
        let encoded = arguments.to_string();
        assert!(!encoded.contains("sk-secret-value-here"));
    }

    #[test]
    fn terminal_failed_carries_redacted_message() {
        let event = EneEvent::Terminal {
            turn: TurnId::new(),
            origin: TurnOrigin::User,
            reason: TerminalReason::Failed {
                message: "auth failed: Bearer abc.def-ghi".into(),
            },
        };
        let public = PublicChatEvent::from_ene_event(&event);
        let PublicChatEvent::Terminal {
            reason, message, ..
        } = public
        else {
            panic!("expected Terminal");
        };
        assert_eq!(reason, "failed");
        let msg = message.expect("message");
        assert!(!msg.contains("abc.def-ghi"));
    }
}
