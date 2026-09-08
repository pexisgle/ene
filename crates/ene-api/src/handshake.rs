//! Pairing, authentication, capability advertisement, and reconnect shapes.
//!
//! Approval and authentication decisions are Host-local: they are made by the
//! Host on its own durable state (Owner confirmation, credential check) and
//! never derived from wire content. A wire message can request, prove, and
//! advertise, but it cannot approve itself.
//!
//! A reconnect is a new connection, not a resumption: it inherits no stream
//! and no round. Prior streams stay closed and prior rounds stay settled; the
//! reconnecting Client re-advertises capabilities and, where needed, asks for
//! history explicitly.

use crate::envelope::ProtocolVersion;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Request to pair a new device with the Host.
///
/// `device_descriptor` is display text for the Owner (for example a device
/// model name shown on the confirmation surface). It carries no authority and
/// is never used as an identifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Human-readable device description for Owner display only.
    pub device_descriptor: String,
}

/// Host-local pairing decision for one [`PairingRequest`].
///
/// Made by the Host (Owner confirmation surface), never derived from wire
/// content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingResult {
    /// Paired; the Client echoes `device_id` on every later message.
    Paired {
        /// Device identifier minted by the Host at pairing.
        device_id: Uuid,
    },
    /// Waiting for the Owner to confirm on the Host surface.
    PendingOwnerConfirmation,
    /// Refused; `reason` is display text only.
    Denied {
        /// Human-readable refusal reason for display only.
        reason: String,
    },
}

/// Authentication challenge issued by the Host.
///
/// `nonce` is a fresh Host-minted challenge string. It is not a secret and
/// not an approval; it only binds the reply to this challenge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    /// Fresh Host-minted challenge the proof must answer.
    pub nonce: String,
}

/// Proof answering an [`AuthChallenge`].
///
/// `proof` demonstrates possession without revealing: it is never a plaintext
/// secret, and no plaintext secret ever crosses this boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthProof {
    /// Challenge answer; never a plaintext secret.
    pub proof: String,
}

/// Host-local authentication decision for one [`AuthProof`].
///
/// Made by the Host on its own credential state, never derived from wire
/// content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthResult {
    /// Authenticated; the Client echoes `connection_id` on later messages.
    Accepted {
        /// Connection identifier minted by the Host at authentication.
        connection_id: Uuid,
    },
    /// Rejected; `reason` is display text only.
    Rejected {
        /// Human-readable rejection reason for display only.
        reason: String,
    },
}

/// Client capability kind the wire can negotiate.
///
/// Extended with new variants in later stages; a Host that does not recognize
/// a variant rejects the message instead of defaulting it to another kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientFeatureKind {
    /// Plain-text conversation.
    Text,
}

/// One Client capability and whether it is currently available.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientFeature {
    /// Capability this entry describes.
    pub kind: ClientFeatureKind,
    /// Whether the Client can use the capability right now.
    pub available: bool,
}

/// Capability advertisement sent by the Client after connecting.
///
/// An advertisement describes; it does not grant. Which features apply is the
/// Host's [`NegotiatedConnection`] decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAdvertise {
    /// Protocol versions the Client can speak, newest first.
    pub supported_protocol: Vec<ProtocolVersion>,
    /// Capabilities the Client implements and their availability.
    pub features: Vec<ClientFeature>,
    /// Human-readable platform description for display only.
    pub platform: String,
}

/// Host-decided outcome of capability negotiation.
///
/// The Host picks the version and the accepted features; the Client must not
/// assume an advertised feature applies until it appears here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedConnection {
    /// Protocol version both sides will speak.
    pub version: ProtocolVersion,
    /// Features the Host accepts for this connection.
    pub accepted_features: Vec<ClientFeatureKind>,
}

/// Hello starting a reconnect.
///
/// Fieldless on purpose: a reconnect carries no stream or round inheritance.
/// The Client re-advertises capabilities with [`CapabilityAdvertise`] and the
/// Host treats the result as a brand-new connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectHello;

/// Invitation to recover, sent by the Host.
///
/// `hint` is Host-supplied display text telling the Client where to look
/// (for example which companion or view to offer). It is display only, never
/// an identifier or a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryInvite {
    /// Display-only hint for the recovery surface.
    pub hint: String,
}

/// Notice that a connection or pairing flow is ending.
///
/// `reason` is display text only; it explains nothing about Host internals
/// and authorizes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectNotice {
    /// Human-readable disconnect reason for display only.
    pub reason: String,
}
