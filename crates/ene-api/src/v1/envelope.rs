//! Routing envelope and protocol version (IPC §5, §7.2).
//!
//! The envelope routes; it never authorizes. Envelope validation success is
//! not payload acceptance: the Host maps the envelope, validates the payload,
//! and hands domain premises to owner checks. There is deliberately no
//! payload field here: payload framing is the transport's job and arrives in
//! Stage 2, when [`super::payload::WirePayload`] becomes the typed body.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::refs::{
    ClientIncarnationId, ConnectionWireId, DeviceWireId, RoundWireId, SpanWireId, TicketWireId,
    WireMessageId, WireMessageType,
};
use super::refs::{CommandWireId, RequestWireId, StreamWireId};

/// Major marks the semantic-compatibility boundary; minor covers
/// backwards-compatible additions (IPC §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// Different majors do not interoperate.
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const V1: Self = Self { major: 1, minor: 0 };

    /// A narrow predicate, not a compatibility verdict: sharing a major only
    /// admits the pair to negotiation. The negotiated version is fixed per
    /// connection (older minor's understood range, never silent upgrade) in
    /// Stage 2.
    #[must_use]
    pub fn shares_major_with(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

/// Request/response, command/ack, and stream correspondence (IPC §6).
/// Every slot is optional because different patterns use different slots:
/// a stream frame carries no request ID, a fact carries none at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WireCorrelation {
    pub request_id: Option<RequestWireId>,
    pub command_id: Option<CommandWireId>,
    pub stream_id: Option<StreamWireId>,
    /// Message this message answers, for transport pairing.
    pub reply_to: Option<WireMessageId>,
    pub causation_span: Option<SpanWireId>,
}

/// Who sent the message: device, incarnation, connection (IPC §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WireSender {
    /// Pairing link. [`None`] only for the pre-pairing [`super::handshake::PairingRequest`]:
    /// an unpaired Client has no device key yet because the Host issues it
    /// after Owner confirmation (IPC §9.2). Every other Client→Host message
    /// must carry it; absence is rejection, never an unconstrained sender.
    pub device_id: Option<DeviceWireId>,
    /// Client process incarnation, sender-minted per boot.
    pub incarnation_id: ClientIncarnationId,
    /// Host-issued connection key. [`None`] before authentication; later
    /// messages without it are rejected, never treated as pre-auth.
    pub connection_id: Option<ConnectionWireId>,
}

/// What the Client saw when it sent the message: comparison material for
/// the Host, never a claim of currentness (IPC §5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObservedMarks {
    pub presence_generation_view: Option<u64>,
    /// Round the Client believes it belongs to. It must be [`None`]
    /// whenever the input does not join a round — a bare round-less
    /// request or a force-new one — and once a sender populates it on a
    /// submit it must equal the payload
    /// [`round`](crate::v1::round::SubmitTextInput::round) premise: the two
    /// fields carry one premise, and a disagreement is rejected, never
    /// adopted one side over the other. The Host never rebinds an old
    /// round from this field.
    pub round_view: Option<RoundWireId>,
    pub ticket_view: Option<TicketWireId>,
}

/// `message_type` names the payload shape for routing; an unknown value is
/// rejected, never guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEnvelope {
    pub protocol: ProtocolVersion,
    pub message_id: WireMessageId,
    pub correlation: WireCorrelation,
    pub sender: WireSender,
    pub observed: ObservedMarks,
    pub message_type: WireMessageType,
}

#[must_use]
pub fn new_outgoing_envelope(
    protocol: ProtocolVersion,
    sender: WireSender,
    message_type: WireMessageType,
) -> WireEnvelope {
    WireEnvelope {
        protocol,
        message_id: WireMessageId(Uuid::new_v4()),
        correlation: WireCorrelation {
            request_id: None,
            command_id: None,
            stream_id: None,
            reply_to: None,
            causation_span: None,
        },
        sender,
        observed: ObservedMarks {
            presence_generation_view: None,
            round_view: None,
            ticket_view: None,
        },
        message_type,
    }
}

#[cfg(test)]
mod tests {
    use super::super::refs::ClientIncarnationId;
    use super::WireSender;
    use super::{ProtocolVersion, WireMessageType, new_outgoing_envelope};

    #[test]
    fn major_match_admits_negotiation_only() {
        assert!(ProtocolVersion::V1.shares_major_with(&ProtocolVersion { major: 1, minor: 9 }));
        assert!(!ProtocolVersion::V1.shares_major_with(&ProtocolVersion { major: 0, minor: 0 }));
    }

    #[test]
    fn fresh_envelopes_carry_distinct_message_ids() {
        let sender = WireSender {
            device_id: None,
            incarnation_id: ClientIncarnationId {
                counter: 0,
                random: 1,
            },
            connection_id: None,
        };
        let first =
            new_outgoing_envelope(ProtocolVersion::V1, sender, WireMessageType(String::new()));
        let second =
            new_outgoing_envelope(ProtocolVersion::V1, sender, WireMessageType(String::new()));
        assert_ne!(first.message_id, second.message_id);
    }
}
