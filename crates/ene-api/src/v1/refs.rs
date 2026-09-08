//! Opaque wire identifiers and references.
//!
//! Every ID-like value on the wire is a dedicated newtype, even when several
//! share [`uuid::Uuid`] or [`String`] underneath. Roles, lifetimes, and
//! issuers differ (IPC §6.1), so sharing one primitive would let a round be
//! passed where a stream is expected. No `From` conversions or cross-type
//! comparisons exist between these types; the Host maps each to its own
//! domain newtype at ingress.
//!
//! Clients echo references back; they never parse, synthesize, or store them
//! as primary keys.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Macro-free helper: Uuid-backed wire IDs share derives and documentation
/// shape. Each expansion below is still its own type with no conversions.
macro_rules! uuid_wire_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);
    };
}

/// Macro-free helper: String-backed wire references share derives and
/// documentation shape. Each expansion is still its own opaque type.
macro_rules! string_wire_ref {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);
    };
}

uuid_wire_id!(
    WireMessageId,
    "Per-send transport ID, minted fresh by the sender. Duplicate suppression only; never a domain identity."
);
uuid_wire_id!(
    RequestWireId,
    "Request/response pair key, minted by the request sender."
);
uuid_wire_id!(
    CommandWireId,
    "Command/ack saga key and domain idempotency key. Transport retry reuses it with a new message ID."
);
uuid_wire_id!(
    StreamWireId,
    "Stream key, issued by the Host by default. Frames of one stream never move to another."
);
uuid_wire_id!(
    ConnectionWireId,
    "Connection key, issued by the Host on authentication success. Reconnect mints a new one."
);
uuid_wire_id!(
    DeviceWireId,
    "Device key, issued by the Host after Owner-confirmed pairing. See the bootstrap rule on `super::envelope::WireSender`."
);

/// Client process incarnation: a different dimension from connection
/// identity, Client instance, and presence generation (IPC §6.3, §11).
/// The three must never collapse into one session ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientIncarnationId {
    /// Sender-side sequence, disambiguating restarts of one device.
    pub counter: u64,
    /// Sender-side randomness, disambiguating colliding counters.
    pub random: u64,
}

string_wire_ref!(
    CompanionWireRef,
    "Opaque Companion reference, Host-issued. Echo only."
);
string_wire_ref!(
    ClientWireRef,
    "Opaque Client reference, Host-issued. Echo only."
);
string_wire_ref!(
    RoundWireId,
    "Opaque round reference, Host-issued. Old rounds are never rebound to new ones."
);
string_wire_ref!(
    TicketWireId,
    "Opaque ticket reference (capture and similar), Host-issued. Echo only."
);
string_wire_ref!(
    SpanWireId,
    "Diagnostic/tracing span reference. Never authority, never ordering evidence."
);
string_wire_ref!(
    ClientLocalId,
    "Client-local correspondence ID (e.g. matching an input to its ack). Monotonic within the Client; never Host-canonical. Named after IPC §6.1/§13.1; §21 pseudo-code spells it `ClientLocalId` for the same role."
);
string_wire_ref!(
    TextLangWire,
    "Opaque language tag (e.g. BCP-47) for a text body. Display/routing hint only."
);
string_wire_ref!(
    RevalidationReasonWire,
    "Opaque revalidation reason. The Host matches it against a known set at ingress (Stage 2); unknown values are rejected, never defaulted."
);
string_wire_ref!(
    ViewMarkWire,
    "Opaque display-revision mark for a management view."
);
string_wire_ref!(
    WireMessageType,
    "Payload discriminator for routing. The Host rejects unknown values as `UnsupportedMessage` (Stage 2); senders never guess."
);
string_wire_ref!(
    ManagementTargetWire,
    "Opaque management target reference: wire refs only, never control state."
);
string_wire_ref!(
    BaseViewMark,
    "Opaque display-revision mark an intent was built on. Comparison material, never authority."
);
