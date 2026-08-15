//! Tool-permission audit log domain model.
//!
//! Every tool invocation is recorded with its permission decision and outcome.
//! Argument payloads are redacted before persistence so secrets (API keys,
//! raw prompt text) never land in the audit trail.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditDecision {
    /// No permission prompt was required (read-only / pre-approved).
    NotRequired,
    /// The user approved the action for this single call.
    AllowOnce,
    /// The user approved the action for the session (pattern grant).
    AllowSession,
    /// The user (or fail-closed policy) denied the action.
    Denied,
}

impl AuditDecision {
    /// Stable string contract stored in the `decision` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::AllowOnce => "allow_once",
            Self::AllowSession => "allow_session",
            Self::Denied => "denied",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "allow_once" => Self::AllowOnce,
            "allow_session" => Self::AllowSession,
            "denied" => Self::Denied,
            _ => Self::NotRequired,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewAuditEntry {
    pub turn_id: String,
    /// Session that triggered the call (`None` for out-of-band diagnostics calls).
    pub session_id: Option<String>,
    /// Namespaced tool name (e.g. `fs.write_file`).
    pub tool_name: String,
    /// Action label reported by the permission prompt (may be empty).
    pub action: String,
    /// Target resource reported by the permission prompt (may be empty).
    pub target: String,
    pub decision: AuditDecision,
    pub success: bool,
    /// Raw argument JSON; redacted before persistence.
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub turn_id: String,
    /// Session that triggered the call (`None` for rows written before
    /// the `session_id` column existed).
    pub session_id: Option<String>,
    pub tool_name: String,
    pub action: String,
    pub target: String,
    pub decision: AuditDecision,
    pub success: bool,
    pub redacted_args: String,
    pub created_at: DateTime<Utc>,
}

/// Argument keys whose values are always masked in the audit trail.
const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "token",
    "access_token",
    "secret",
    "password",
    "passwd",
    "credential",
    "credentials",
    "authorization",
    "private_key",
    "prompt",
    "content",
];

/// Tool namespaces whose arguments carry personal event content (calendar
/// event titles, notes, attendee addresses, locations).
const EVENT_CONTENT_TOOL_PREFIXES: &[&str] = &["calendar."];

/// Argument keys masked for event-content tools only; the keys are too
/// generic to redact globally (e.g. `title` on a file-metadata tool).
const EVENT_CONTENT_KEYS: &[&str] = &["title", "description", "location", "attendees"];

/// Redacts sensitive values from a JSON argument payload.
///
/// Object values under a sensitive key are replaced with `"[redacted]"`.
/// Non-object payloads (or parse failures) are summarized to their length so
/// raw prompt text never lands in the audit trail.
#[must_use]
pub fn redact_arguments(arguments: &str) -> String {
    redact_arguments_for_tool("", arguments)
}

/// Like [`redact_arguments`], additionally masking event-content keys when
/// `tool_name` belongs to an event-content namespace (e.g. `calendar.*`).
#[must_use]
pub fn redact_arguments_for_tool(tool_name: &str, arguments: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return format!("[non-json args, {} bytes]", arguments.len());
    };
    let redact_event_content = EVENT_CONTENT_TOOL_PREFIXES
        .iter()
        .any(|prefix| tool_name.starts_with(prefix));
    redact_value(&mut value, redact_event_content);
    serde_json::to_string(&value).unwrap_or_else(|_| "[unserializable args]".to_string())
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|k| lower.contains(k))
}

fn is_event_content_key(key: &str) -> bool {
    EVENT_CONTENT_KEYS
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k))
}

fn redact_value(value: &mut serde_json::Value, redact_event_content: bool) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_sensitive_key(key) || (redact_event_content && is_event_content_key(key)) {
                    *val = serde_json::Value::String("[redacted]".to_string());
                } else {
                    redact_value(val, redact_event_content);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item, redact_event_content);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_keys() {
        let args = r#"{"path":"/tmp/x","api_key":"sk-123","nested":{"password":"hunter2"}}"#;
        let redacted = redact_arguments(args);
        assert!(redacted.contains("[redacted]"));
        assert!(!redacted.contains("sk-123"));
        assert!(!redacted.contains("hunter2"));
        assert!(redacted.contains("/tmp/x"));
    }

    #[test]
    fn redacts_prompt_content() {
        let args = r#"{"prompt":"secret instructions"}"#;
        let redacted = redact_arguments(args);
        assert!(!redacted.contains("secret instructions"));
    }

    #[test]
    fn non_json_is_summarized() {
        let redacted = redact_arguments("not json at all");
        assert!(redacted.starts_with("[non-json args"));
    }

    #[test]
    fn calendar_event_content_is_redacted_for_calendar_tools() {
        let args = r#"{"calendar_id":"cal-1","title":"Board meeting","description":"revenue plans","location":"HQ","attendees":["alice@example.com"]}"#;
        let redacted = redact_arguments_for_tool("calendar.create_event", args);
        assert!(!redacted.contains("Board meeting"));
        assert!(!redacted.contains("revenue plans"));
        assert!(!redacted.contains("HQ"));
        assert!(!redacted.contains("alice@example.com"));
        assert!(redacted.contains("cal-1"), "ids stay visible");
    }

    #[test]
    fn calendar_event_content_is_redacted_in_nested_args() {
        let args = r#"{"event":{"title":"secret title","attendees":["a@b.c"]},"ok":true}"#;
        let redacted = redact_arguments_for_tool("calendar.update_event", args);
        assert!(!redacted.contains("secret title"));
        assert!(!redacted.contains("a@b.c"));
        assert!(redacted.contains("true"));
    }

    #[test]
    fn event_content_keys_stay_visible_for_other_tools() {
        let args = r#"{"title":"Board meeting","description":"revenue plans","location":"HQ","attendees":["alice@example.com"]}"#;
        let redacted = redact_arguments_for_tool("notes.write", args);
        assert!(redacted.contains("Board meeting"));
        assert!(redacted.contains("revenue plans"));
        assert!(redacted.contains("alice@example.com"));
    }

    #[test]
    fn generic_redaction_is_unchanged_for_calendar_tools() {
        let args = r#"{"title":"Board meeting","token":"sk-123"}"#;
        let redacted = redact_arguments_for_tool("calendar.create_event", args);
        assert!(!redacted.contains("sk-123"));
    }

    #[test]
    fn decision_roundtrips() {
        for decision in [
            AuditDecision::NotRequired,
            AuditDecision::AllowOnce,
            AuditDecision::AllowSession,
            AuditDecision::Denied,
        ] {
            assert_eq!(AuditDecision::parse(decision.as_str()), decision);
        }
    }
}
