use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::envelope::ProtocolVersion;
use super::refs::DeviceWireId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PairingRequest {
    pub device_descriptor: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PairingResult {
    PendingOwnerConfirmation { pending_id: String },
    Denied { reason: String },
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct PairingProvisionSecret(String);

impl PairingProvisionSecret {
    #[must_use]
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for PairingProvisionSecret {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PairingProvision {
    pub device_id: DeviceWireId,
    pub pairing_secret: PairingProvisionSecret,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub nonce: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthResult {
    Accepted {
        connection_id: super::refs::ConnectionWireId,
    },
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAdvertise {
    pub supported_protocol: Vec<ProtocolVersion>,
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedConnection {
    pub version: ProtocolVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisconnectNotice {
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::{AuthProof, PairingProvisionSecret};

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
    fn pairing_provision_secret_debug_is_redacted() {
        let secret = PairingProvisionSecret::new(String::from("provision-secret-marker"));
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("provision-secret-marker"));
        assert!(rendered.contains("redacted"));
    }
}
