//! Tool-permission audit log domain model (#177).
//!
//! Every tool invocation is recorded with its permission decision and outcome.
//! Argument payloads are redacted before persistence so secrets (API keys,
//! raw prompt text) never land in the audit trail.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The permission decision recorded for an audited tool call.
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

    /// Parses the stored string back into a decision.
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

/// Input for recording a single audited tool call.
#[derive(Debug, Clone)]
pub struct NewAuditEntry {
    /// Turn that triggered the call (empty for out-of-band diagnostics calls).
    pub turn_id: String,
    /// Namespaced tool name (e.g. `fs.write_file`).
    pub tool_name: String,
    /// Action label reported by the permission prompt (may be empty).
    pub action: String,
    /// Target resource reported by the permission prompt (may be empty).
    pub target: String,
    /// Permission decision applied to the call.
    pub decision: AuditDecision,
    /// Whether the tool call ultimately succeeded.
    pub success: bool,
    /// Raw argument JSON; redacted before persistence.
    pub arguments: String,
}

/// A persisted audit-log row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Row id.
    pub id: i64,
    /// Turn that triggered the call.
    pub turn_id: String,
    /// Namespaced tool name.
    pub tool_name: String,
    /// Action label.
    pub action: String,
    /// Target resource.
    pub target: String,
    /// Permission decision.
    pub decision: AuditDecision,
    /// Whether the call succeeded.
    pub success: bool,
    /// Redacted argument JSON.
    pub redacted_args: String,
    /// When the call was recorded.
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

/// Redacts sensitive values from a JSON argument payload.
///
/// Object values under a sensitive key are replaced with `"[redacted]"`.
/// Non-object payloads (or parse failures) are summarized to their length so
/// raw prompt text never lands in the audit trail.
#[must_use]
pub fn redact_arguments(arguments: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return format!("[non-json args, {} bytes]", arguments.len());
    };
    redact_value(&mut value);
    serde_json::to_string(&value).unwrap_or_else(|_| "[unserializable args]".to_string())
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|k| lower.contains(k))
}

fn redact_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *val = serde_json::Value::String("[redacted]".to_string());
                } else {
                    redact_value(val);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item);
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
