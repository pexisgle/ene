//! Pairing, authentication, and capability shells (IPC §8, §9).
//!
//! Approval and authentication decisions stay Host-local; the wire carries
//! requests, opaque proofs, and outcomes. Secret material travels auth
//! frames only, never general payloads, logs, or Debug output.

use serde::{Deserialize, Serialize};

use super::envelope::ProtocolVersion;
use super::refs::DeviceWireId;

/// Starts pairing for a new device: Owner-confirmable descriptor only.
/// Sent pre-pairing, so its envelope carries no device ID (see the bootstrap
/// rule on [`super::envelope::WireSender`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Display only. Never authority.
    pub device_descriptor: String,
}

/// Pairing outcome: an Ok-side outcome, never a retryable error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PairingResult {
    /// Owner confirmed; the device key is now issued.
    Paired { device_id: DeviceWireId },
    /// Waiting on the Host-local trusted-surface confirmation.
    PendingOwnerConfirmation,
    Denied {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
}

/// Host-minted single-use challenge opening one authentication.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthChallenge {
    /// Single-use; old proofs are never reused.
    pub nonce: String,
}

/// Client ownership proof: demonstrates possession, never carries a
/// plaintext secret.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthProof {
    pub proof: String,
}

impl core::fmt::Debug for AuthProof {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AuthProof")
            .field("proof", &"[redacted]")
            .finish()
    }
}

/// Authentication outcome with the connection key on success.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthResult {
    /// Authenticated; this connection key governs later messages.
    Accepted {
        connection_id: super::refs::ConnectionWireId,
    },
    /// Rejected against the current device-auth store.
    Rejected {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
}

/// Client capability kinds a Client may advertise (IPC §8). Text needs no
/// flag: it is the baseline, not a capability. Claim never equals
/// permission; the Host checks availability separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientFeatureKind {
    Body2D,
    VoiceDuplex,
    ScreenCapture,
    ComputerUse,
    Notification,
    TrayIntegration,
}

/// Per-feature detail (codecs, limits) and structured platform/limits
/// descriptors are Stage 2 negotiation scope; Stage 1 carries the display
/// platform string only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientFeature {
    pub kind: ClientFeatureKind,
    /// Whether the Client claims it right now. Claim is not proof.
    pub available: bool,
}

/// Connection-time capability advertisement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAdvertise {
    pub supported_protocol: Vec<ProtocolVersion>,
    pub features: Vec<ClientFeature>,
    /// OS/device description, display only. Never permission evidence.
    pub platform: String,
}

/// Host-selected connection terms. Stored per connection; version and
/// accepted features never mix with restore or presence generations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedConnection {
    pub version: ProtocolVersion,
    /// Receipt, not permission.
    pub accepted_features: Vec<ClientFeatureKind>,
}

/// Reconnect declaration with fresh authentication. Never a state-restore
/// request: old connections, streams, tickets, and rounds are not inherited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReconnectHello;

/// Post-restart recovery invitation toward the pre-restart Client.
/// Asking for confirmation, not establishing presence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecoveryInvite {
    /// Display hint for the Owner. Never presence evidence.
    pub hint: String,
}

/// Observed disconnection fact, either direction. Never an instant durable
/// purge of attribution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisconnectNotice {
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::{AuthProof, PairingRequest};

    #[test]
    fn auth_proof_debug_redacts_the_proof() {
        let proof = AuthProof {
            proof: String::from("ownership-proof-abc"),
        };
        let rendered = format!("{proof:?}");
        assert!(
            !rendered.contains("ownership-proof-abc"),
            "Debug must not carry the proof: {rendered}"
        );
    }

    #[test]
    fn pairing_request_keeps_display_descriptor() {
        let request = PairingRequest {
            device_descriptor: String::from("Owner laptop"),
        };
        let rendered = format!("{request:?}");
        assert!(
            rendered.contains("Owner laptop"),
            "display descriptor stays visible: {rendered}"
        );
    }
}
