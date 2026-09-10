//! Typed transport rejections (IPC §7.2, §24).
//!
//! Domain rejections stay Ok-side domain outcomes; these reject malformed
//! or unnegotiated WIRE usage without side effects and without closing the
//! connection (unless the connection itself is unusable). Never a retry
//! signal: resending the same bytes fails identically.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    /// Unknown message type within the negotiated range.
    UnsupportedMessage,
    /// Unknown enum variant in a closed wire enum.
    UnsupportedFieldValue,
    MissingRequiredField,
    /// No common major version; the connection cannot proceed.
    IncompatibleProtocol,
    /// A reused command key arrived with a different request than the
    /// stored row. The durable key already owns its request fingerprint, so
    /// the conflicting send is refused without side effects; retrying the
    /// same bytes fails identically, while retrying the original request
    /// replays cleanly.
    ConflictingCommand,
}

/// One rejected message: kind plus operational detail only (never secrets,
/// bodies, or frame bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RejectNotice {
    pub kind: RejectKind,
    /// Operational detail (offending type name, version pair, ...).
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::{RejectKind, RejectNotice};

    #[test]
    fn reject_roundtrips_through_json() {
        let notice = RejectNotice {
            kind: RejectKind::UnsupportedMessage,
            detail: String::from("Ping"),
        };
        let json = serde_json::to_string(&notice);
        assert!(json.is_ok(), "reject must serialize");
        let Some(json) = json.ok() else {
            return;
        };
        let back: Result<RejectNotice, _> = serde_json::from_str(&json);
        assert!(back.is_ok(), "reject must deserialize");
        let Some(back) = back.ok() else {
            return;
        };
        assert_eq!(back, notice, "reject must roundtrip");
    }

    #[test]
    fn reject_kinds_are_distinct_outcomes() {
        assert_ne!(
            RejectKind::UnsupportedMessage,
            RejectKind::IncompatibleProtocol
        );
        assert_ne!(
            RejectKind::UnsupportedFieldValue,
            RejectKind::MissingRequiredField
        );
        assert_ne!(
            RejectKind::ConflictingCommand,
            RejectKind::UnsupportedMessage
        );
    }
}
