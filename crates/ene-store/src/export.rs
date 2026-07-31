//! Versioned session export format (#176).
//!
//! A [`SessionExport`] bundles a session's metadata, conversation messages,
//! and tool-audit logs into a single serializable document for backup or
//! transfer. Secrets are redacted before serialization: tool arguments reuse
//! [`crate::audit::redact_arguments`], and message content is passed through
//! [`redact_secrets`] so obvious credentials (API keys, bearer tokens, PEM
//! private-key blocks) never leave the store in the clear.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::EneMemoryError;
use crate::session::SessionMeta;

/// Current session-export format version.
///
/// Bumped whenever the on-disk JSON shape changes in a breaking way.
/// [`SessionExport::from_json`] rejects any other version.
pub const SESSION_EXPORT_FORMAT_VERSION: u32 = 1;

/// Placeholder substituted for any redacted secret material.
const REDACTED: &str = "[redacted]";

/// A complete, self-contained export of one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    /// Format version; must equal [`SESSION_EXPORT_FORMAT_VERSION`].
    pub format_version: u32,
    /// When the export was produced.
    pub exported_at: DateTime<Utc>,
    /// Session metadata.
    pub session: SessionMeta,
    /// Conversation messages, oldest first.
    pub messages: Vec<ExportedMessage>,
    /// Tool-audit log entries associated with the session (may be empty).
    pub tool_logs: Vec<ExportedToolLog>,
}

impl SessionExport {
    /// Serializes the export to pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns [`EneMemoryError::SerializationError`] if serialization fails.
    pub fn to_json(&self) -> Result<String, EneMemoryError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parses an export from JSON, rejecting unknown format versions.
    ///
    /// # Errors
    ///
    /// Returns [`EneMemoryError::SerializationError`] on malformed JSON and
    /// [`EneMemoryError::UnsupportedFormatVersion`] when `format_version` does
    /// not equal [`SESSION_EXPORT_FORMAT_VERSION`].
    pub fn from_json(json: &str) -> Result<Self, EneMemoryError> {
        let export: Self = serde_json::from_str(json)?;
        if export.format_version != SESSION_EXPORT_FORMAT_VERSION {
            return Err(EneMemoryError::UnsupportedFormatVersion(
                export.format_version,
            ));
        }
        Ok(export)
    }
}

/// A single exported conversation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedMessage {
    /// Role that produced the message (e.g. `user`, `assistant`).
    pub role: String,
    /// Message content (secrets redacted).
    pub content: String,
    /// When the message was recorded.
    pub created_at: DateTime<Utc>,
}

/// A single exported tool-audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedToolLog {
    /// Turn that triggered the call.
    pub turn_id: String,
    /// Namespaced tool name.
    pub tool_name: String,
    /// Action label.
    pub action: String,
    /// Target resource.
    pub target: String,
    /// Permission decision string.
    pub decision: String,
    /// Whether the call succeeded.
    pub success: bool,
    /// Redacted argument JSON.
    pub redacted_args: String,
    /// When the call was recorded.
    pub created_at: DateTime<Utc>,
}

/// Masks obvious secrets in free-form message content.
///
/// This is a **best-effort safety net, not a primary defense**: it recognizes
/// a small set of high-signal credential shapes and leaves everything else
/// untouched, so it must not be relied upon to keep arbitrary secrets out of
/// an export. Secrets should never live in message content in the first
/// place; this only catches the ones that slip through.
///
/// Recognized shapes:
///
/// - `-----BEGIN ... PRIVATE KEY-----` … `-----END ... PRIVATE KEY-----` PEM
///   blocks (replaced wholesale),
/// - `Bearer <token>` authorization tokens, and
/// - API keys carrying a known provider prefix: `sk-` (`OpenAI`-style), `AIza`
///   (Google), `AKIA` (AWS access key ID), and `GitHub`'s `ghp_` / `gho_` /
///   `ghu_` / `ghs_` / `ghr_` / `github_pat_` tokens.
///
/// Prefixes only match at a word boundary, so ordinary words such as
/// "risk-free" are left untouched. Credentials that do not use one of these
/// shapes — or that use a prefix not listed above — pass through unchanged.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let without_pem = redact_pem_blocks(input);
    let mut out = String::with_capacity(without_pem.len());
    let mut rest = without_pem.as_str();
    while !rest.is_empty() {
        let bearer = find_bearer(rest);
        let api_key = find_api_key(rest);
        let (next, is_bearer) = match (bearer, api_key) {
            (Some(b), Some(a)) => {
                if b <= a {
                    (b, true)
                } else {
                    (a, false)
                }
            }
            (Some(b), None) => (b, true),
            (None, Some(a)) => (a, false),
            (None, None) => {
                out.push_str(rest);
                break;
            }
        };
        out.push_str(rest.get(..next).unwrap_or_default());
        let from = rest.get(next..).unwrap_or_default();
        let consumed = if is_bearer {
            mask_bearer(from)
        } else {
            mask_api_key(from)
        };
        out.push_str(REDACTED);
        rest = from.get(consumed..).unwrap_or_default();
    }
    out
}

/// Replaces each PEM private-key block (begin marker through end marker) with
/// the redaction placeholder. Unterminated blocks redact to the end of input.
fn redact_pem_blocks(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(begin) = rest.find("-----BEGIN") {
        out.push_str(rest.get(..begin).unwrap_or_default());
        let from_begin = rest.get(begin..).unwrap_or_default();
        let end = pem_block_end(from_begin);
        out.push_str(REDACTED);
        rest = from_begin.get(end..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// Returns the byte offset just past the end of the PEM block that starts at
/// the head of `text` (which begins with `"-----BEGIN"`). If no closing
/// `-----END ... -----` marker is found, returns `text.len()` (redact to end).
fn pem_block_end(text: &str) -> usize {
    let Some(end_start) = text.find("-----END") else {
        return text.len();
    };
    // Skip the leading "-----" of the END marker, then find the closing "-----".
    let after_prefix = end_start.saturating_add(5);
    text.get(after_prefix..)
        .and_then(|tail| tail.find("-----"))
        .map_or(text.len(), |off| {
            after_prefix.saturating_add(off).saturating_add(5)
        })
}

/// Finds the byte offset of the next word-boundary `Bearer ` token in `text`.
fn find_bearer(text: &str) -> Option<usize> {
    find_at_boundary(text, "Bearer ", 7)
}

/// API key prefixes recognized by [`redact_secrets`], covering the major
/// cloud providers plus `GitHub`. The list is intentionally conservative: each
/// entry is a distinctive, low-false-positive prefix rather than a guess.
const API_KEY_PREFIXES: &[&str] = &[
    "sk-",         // OpenAI-style secret keys
    "AIza",        // Google API keys
    "AKIA",        // AWS access key IDs
    "ghp_",        // GitHub personal access tokens
    "gho_",        // GitHub OAuth tokens
    "ghu_",        // GitHub user-to-server tokens
    "ghs_",        // GitHub server-to-server tokens
    "ghr_",        // GitHub refresh tokens
    "github_pat_", // GitHub fine-grained personal access tokens
];

/// Finds the byte offset of the next word-boundary API key in `text`, matching
/// any of [`API_KEY_PREFIXES`]. Returns the earliest match across all prefixes.
fn find_api_key(text: &str) -> Option<usize> {
    API_KEY_PREFIXES
        .iter()
        .copied()
        .filter_map(|prefix| find_at_boundary(text, prefix, prefix.len()))
        .min()
}

/// Finds the next occurrence of `needle` in `text` that is preceded by a word
/// boundary (start of string or a non-alphanumeric character). `needle_len`
/// is used to advance past false positives.
fn find_at_boundary(text: &str, needle: &str, needle_len: usize) -> Option<usize> {
    let mut search_from = 0;
    while let Some(rel) = text.get(search_from..).and_then(|s| s.find(needle)) {
        let pos = search_from.saturating_add(rel);
        let prev = text.get(..pos).and_then(|s| s.chars().next_back());
        let at_boundary = prev.is_none_or(|c| !c.is_alphanumeric());
        if at_boundary {
            return Some(pos);
        }
        search_from = pos.saturating_add(needle_len);
    }
    None
}

/// Returns the byte length of the `Bearer <token>` run starting at the head of
/// `text` (which must begin with `"Bearer "`), so the caller can mask it.
fn mask_bearer(text: &str) -> usize {
    let after = text.strip_prefix("Bearer ").unwrap_or_default();
    let token_len = after
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after.len());
    text.len()
        .saturating_sub(after.len())
        .saturating_add(token_len)
}

/// Returns the byte length of the API key starting at the head of `text`
/// (which begins with one of [`API_KEY_PREFIXES`]), so the caller can mask it.
/// Keys end at the first whitespace or quote.
fn mask_api_key(text: &str) -> usize {
    text.find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta(session_id: &str) -> SessionMeta {
        let now = Utc::now();
        SessionMeta {
            id: 1,
            session_id: session_id.to_string(),
            card_name: "test-card".to_string(),
            title: "Test Session".to_string(),
            created_at: now,
            updated_at: now,
            archived: false,
            turn_count: 2,
        }
    }

    #[test]
    fn redacts_sk_api_keys() {
        let out = redact_secrets("my key is sk-abc123XYZ_ok thanks");
        assert!(!out.contains("sk-abc123XYZ_ok"), "key leaked: {out}");
        assert!(out.contains("[redacted]"));
        assert!(out.contains("my key is"));
        assert!(out.contains("thanks"));
    }

    #[test]
    fn redacts_bearer_tokens() {
        let out = redact_secrets("Authorization: Bearer abc.def-ghi end");
        assert!(!out.contains("abc.def-ghi"), "token leaked: {out}");
        assert!(out.contains("Authorization:"));
        assert!(out.contains("end"));
    }

    #[test]
    fn redacts_cloud_provider_keys() {
        // Google, AWS, and GitHub prefixes are redacted like `sk-` keys (#427
        // item 7).
        for secret in [
            "AIzaSyD-secret-google-key",
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_1234567890abcdefghij",
            "github_pat_11ABCDEFG_secret",
        ] {
            let input = format!("token is {secret} here");
            let out = redact_secrets(&input);
            assert!(!out.contains(secret), "secret leaked: {out}");
            assert!(out.contains("[redacted]"), "no redaction marker: {out}");
            assert!(out.contains("token is"), "prose lost: {out}");
            assert!(out.contains("here"), "prose lost: {out}");
        }
    }

    #[test]
    fn redacts_pem_private_key_blocks() {
        let input = "prefix -----BEGIN RSA PRIVATE KEY-----\nMIIBsecretdata\n-----END RSA PRIVATE KEY----- suffix";
        let out = redact_secrets(input);
        assert!(!out.contains("MIIBsecretdata"), "pem body leaked: {out}");
        assert!(!out.contains("PRIVATE KEY"), "pem marker leaked: {out}");
        assert!(out.contains("prefix"));
        assert!(out.contains("suffix"));
    }

    #[test]
    fn preserves_normal_text() {
        let input = "Hello! This is a normal message about risk-free skating and desk work.";
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn export_json_roundtrip() {
        let now = Utc::now();
        let export = SessionExport {
            format_version: SESSION_EXPORT_FORMAT_VERSION,
            exported_at: now,
            session: sample_meta("sess-1"),
            messages: vec![ExportedMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                created_at: now,
            }],
            tool_logs: vec![ExportedToolLog {
                turn_id: "t1".to_string(),
                tool_name: "fs.write_file".to_string(),
                action: "write".to_string(),
                target: "/tmp/x".to_string(),
                decision: "allow_once".to_string(),
                success: true,
                redacted_args: "{}".to_string(),
                created_at: now,
            }],
        };

        let json = export.to_json().unwrap();
        let restored = SessionExport::from_json(&json).unwrap();
        assert_eq!(restored.session.session_id, "sess-1");
        assert_eq!(restored.messages.len(), 1);
        assert_eq!(restored.tool_logs.len(), 1);
        assert_eq!(restored.format_version, SESSION_EXPORT_FORMAT_VERSION);
    }

    #[test]
    fn rejects_unknown_format_version() {
        let json = r#"{"format_version":999,"exported_at":"2026-01-01T00:00:00Z","session":{"id":1,"session_id":"s","card_name":"","title":"","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","archived":false,"turn_count":0},"messages":[],"tool_logs":[]}"#;
        let err = SessionExport::from_json(json).unwrap_err();
        assert!(matches!(err, EneMemoryError::UnsupportedFormatVersion(999)));
    }
}
