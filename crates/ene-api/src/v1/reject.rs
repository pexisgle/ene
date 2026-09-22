//! Typed transport rejections (IPC §7.2, §24).
//!
//! Domain rejections stay Ok-side domain outcomes; these reject malformed
//! or unnegotiated WIRE usage without side effects. A [`RejectNotice`] is
//! frame-level and keeps the connection unless the connection itself is
//! unusable; [`IncompatibleProtocol`] answers a major mismatch — in the
//! envelope or in capability negotiation — and is terminal. Never a retry
//! signal: resending the same bytes fails identically.

use serde::{Deserialize, Serialize};

use super::envelope::ProtocolVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    UnsupportedMessage,
    UnsupportedFieldValue,
    MissingRequiredField,
    /// The message's protocol version cannot be processed: no shared major at
    /// negotiation, or an envelope outside the connection's negotiated
    /// version.
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

/// Terminal protocol refusal: no common major version (IPC §7.2, V-11).
///
/// Answers a major mismatch — capability negotiation with no shared major, or
/// a pre-negotiation envelope the Host's major cannot interpret — and the
/// connection ends after it is written. The Host reports both sides' maxima
/// plus operator guidance, so the Client can distinguish "upgrade the Client"
/// from "this Host is older" and never guesses compatibility. Never a retry
/// signal: no retry makes the majors intersect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IncompatibleProtocol {
    /// Highest protocol version the Host supports.
    pub host_max: ProtocolVersion,
    /// Highest protocol version the Client advertised.
    pub client_max: ProtocolVersion,
    /// Upgrade guidance naming the protocol major to move to. Operational
    /// text only.
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
