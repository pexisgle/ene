use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    UnsupportedMessage,
    UnsupportedFieldValue,
    MissingRequiredField,
    IncompatibleProtocol,
    /// The connection was superseded by a newer authentication for the same
    /// device. The socket stays open (IPC §11.3), but the connection can never
    /// become current again: further service requires a new connection.
    StaleConnection,
    InvalidHandshakePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RejectNotice {
    pub kind: RejectKind,
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
            RejectKind::InvalidHandshakePhase,
            RejectKind::StaleConnection
        );
    }
}
