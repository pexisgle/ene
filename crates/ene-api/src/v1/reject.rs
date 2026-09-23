use serde::{Deserialize, Serialize};

use super::envelope::ProtocolVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    UnsupportedMessage,
    UnsupportedFieldValue,
    MissingRequiredField,
    IncompatibleProtocol,
    StaleConnection,
    InvalidHandshakePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RejectNotice {
    pub kind: RejectKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IncompatibleProtocol {
    pub host_max: ProtocolVersion,
    pub client_max: ProtocolVersion,
    pub hint: String,
}

#[cfg(test)]
mod tests {
    use super::{IncompatibleProtocol, RejectKind, RejectNotice};
    use crate::v1::envelope::ProtocolVersion;

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

    #[test]
    fn incompatible_protocol_roundtrips_with_both_maxima_and_hint() {
        let rejection = IncompatibleProtocol {
            host_max: ProtocolVersion::V1,
            client_max: ProtocolVersion { major: 9, minor: 3 },
            hint: String::from("use a client release sharing the host's protocol major 1"),
        };
        let json = serde_json::to_string(&rejection);
        assert!(json.is_ok(), "incompatible protocol must serialize");
        let Some(json) = json.ok() else {
            return;
        };
        let back: Result<IncompatibleProtocol, _> = serde_json::from_str(&json);
        assert!(back.is_ok(), "incompatible protocol must deserialize");
        let Some(back) = back.ok() else {
            return;
        };
        assert_eq!(back, rejection, "both maxima and the hint must roundtrip");
    }
}
