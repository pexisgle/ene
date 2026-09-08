//! Routing-only wire envelope: compatibility, correlation, and sender marks.
//!
//! The envelope carries what the transport needs to route, deduplicate, and
//! version-check a message. It never carries authority: a well-formed envelope
//! is not payload acceptance, and nothing here decides domain meaning. The
//! Host maps the envelope, then validates the payload and hands domain
//! premises to each owning crate for its own check.
//!
//! There is deliberately no payload field on
//! [`crate::envelope::WireEnvelope`]: payload framing
//! is the transport's job and arrives in Stage 2. This crate fixes module
//! layout and field shapes only.
//!
//! `message_type` is a routing hint, not a meaning. An unknown `message_type`
//! is rejected by the Host (Stage 2) — never guessed, never defaulted to
//! another payload shape.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Wire protocol version carried on every envelope.
///
/// `major` is the semantic-compatibility boundary: versions with different
/// majors cannot interoperate. `minor` covers backwards-compatible additions
/// such as new optional fields within the same major.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// Semantic-compatibility boundary; must match for interop.
    pub major: u16,
    /// Backwards-compatible additions within the same major.
    pub minor: u16,
}

impl ProtocolVersion {
    /// First stable wire version of the Stage 1 contract.
    ///
    /// ```
    /// use ene_api::envelope::ProtocolVersion;
    ///
    /// assert_eq!(ProtocolVersion::V1.major, 1);
    /// assert_eq!(ProtocolVersion::V1.minor, 0);
    /// ```
    pub const V1: Self = Self { major: 1, minor: 0 };

    /// Reports whether `other` can interoperate with `self`.
    ///
    /// Compatibility requires an equal `major`; `minor` may differ. A
    /// different major is a required semantic change and must be rejected,
    /// never guessed around.
    ///
    /// ```
    /// use ene_api::envelope::ProtocolVersion;
    ///
    /// let minor_bump = ProtocolVersion { major: 1, minor: 4 };
    /// assert!(ProtocolVersion::V1.is_compatible_with(&minor_bump));
    /// let next_major = ProtocolVersion { major: 2, minor: 0 };
    /// assert!(!ProtocolVersion::V1.is_compatible_with(&next_major));
    /// ```
    #[must_use]
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

/// Request/response, command/ack, and stream correspondence for one message.
///
/// Every identifier here is minted by the sender for transport correspondence
/// only; none is a domain identity. A field is [`None`] when the message
/// takes part in no saga of that kind. A transport retry keeps the same
/// `command_id` and mints a fresh [`WireEnvelope::message_id`]; the receiver
/// tells duplicate delivery (same `message_id`) apart from domain idempotency
/// (same `command_id`) and never re-executes either.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireCorrelation {
    /// Request/response pair key, minted by the request sender.
    pub request_id: Option<Uuid>,
    /// Command/ack saga key and domain idempotency key, minted by the command
    /// sender.
    pub command_id: Option<Uuid>,
    /// Stream this message belongs to, minted by the stream opener.
    pub stream_id: Option<Uuid>,
    /// Message this message answers, for transport pairing. Domain
    /// correspondence uses the saga identifiers above, never this field.
    pub reply_to: Option<Uuid>,
}

/// Who sent the message: device, process boot, and connection.
///
/// `device_id` is bound at pairing and echoed by the Client on every message.
/// `incarnation` is a sender-minted boot nonce: the Client picks a fresh value
/// at each process boot so the Host can reject messages from a stale process
/// generation. `connection_id` is minted by the Host at authentication and is
/// [`None`] before authentication; domain operations require it (enforced in
/// Stage 2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireSender {
    /// Pairing-bound device identifier, echoed on every message.
    pub device_id: Uuid,
    /// Sender-minted boot nonce; fresh at each Client process boot.
    pub incarnation: u64,
    /// Host-minted connection identifier; [`None`] before authentication.
    pub connection_id: Option<Uuid>,
}

/// Copies of the generation and round marks the Client last saw.
///
/// These are observations, not claims: the Client being convinced it is
/// current settles nothing. The Host re-checks both marks against durable and
/// live state and treats a mismatch as stale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedMarks {
    /// Presence generation value the Client last saw, if any.
    pub presence_generation_view: Option<u64>,
    /// Round reference the Client believes it belongs to, if any. Opaque:
    /// the Client must echo it, never parse, synthesize, or store it as a key.
    pub round_view: Option<String>,
}

/// Routing-only envelope for one wire message.
///
/// The envelope routes; it never authorizes. There is no payload field:
/// payload framing is the transport's job and arrives in Stage 2.
/// `message_type` names the payload shape for routing; an unknown value is
/// rejected by the Host (Stage 2), never guessed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEnvelope {
    /// Protocol version of the sender; checked with
    /// [`ProtocolVersion::is_compatible_with`].
    pub protocol: ProtocolVersion,
    /// Fresh identifier minted per send, including retries. The receiver's
    /// duplicate-suppression key only; never a domain identity.
    pub message_id: Uuid,
    /// Saga correspondence for this message.
    pub correlation: WireCorrelation,
    /// Device, boot nonce, and connection of the sender.
    pub sender: WireSender,
    /// Generation and round marks the sender last saw.
    pub observed: ObservedMarks,
    /// Payload shape name for routing. Unknown values are rejected by the
    /// Host (Stage 2), never guessed.
    pub message_type: String,
}
