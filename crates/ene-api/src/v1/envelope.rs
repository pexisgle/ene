//! Routing envelope and protocol version (IPC §5, §7.2).
//!
//! The envelope routes; it never authorizes. Envelope validation success is
//! not payload acceptance: the Host maps the envelope, validates the payload,
//! and hands domain premises to owner checks. There is deliberately no
//! payload field here: payload framing is the transport frame's job, and
//! [`super::payload::WirePayload`] is the typed body carried beside the
//! envelope.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::refs::{
    ClientIncarnationId, CommandWireId, ConnectionWireId, DeviceWireId, RequestWireId, RoundWireId,
    WireMessageId, WireMessageType,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const V1: Self = Self { major: 1, minor: 0 };

    /// A narrow predicate, not a compatibility verdict: sharing a major only
    /// admits the pair to negotiation. The negotiated version is fixed per
    /// connection (older minor's understood range, never silent upgrade).
    #[must_use]
    pub fn shares_major_with(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WireCorrelation {
    pub request_id: Option<RequestWireId>,
    pub command_id: Option<CommandWireId>,
    pub reply_to: Option<WireMessageId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WireSender {
    pub device_id: Option<DeviceWireId>,
    pub incarnation_id: ClientIncarnationId,
    pub connection_id: Option<ConnectionWireId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObservedMarks {
    pub presence_generation_view: Option<u64>,
    pub round_view: Option<RoundWireId>,
}

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
            reply_to: None,
        },
        sender,
        observed: ObservedMarks {
            presence_generation_view: None,
            round_view: None,
        },
        message_type,
    }
}
