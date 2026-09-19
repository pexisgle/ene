//! Host-local control DTOs. Not on `ene-api`, not remote-capable.
//!
//! Secret fields use [`RedactedSecret`]: `Debug` never prints the raw value.
//! Confirmation completion is a seat-bound session id plus freshness nonce.
//! Empty-seat first-come is not authenticity evidence.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Volatile secret carried only on the control intake path.
///
/// Serialized for the Host-local frame; never shown by `Debug`. Drop
/// zeroizes the buffer. This type is not an `ene-api` payload.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct RedactedSecret(String);

impl core::fmt::Debug for RedactedSecret {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl RedactedSecret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        let mut owned = self;
        core::mem::take(&mut owned.0)
    }
}

/// Operation named in a confirmation challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlOp {
    DeviceApprove,
    CredentialPut,
    DeletionConfirm,
}

/// Body-free preview of one staged Targeted Deletion request.
///
/// The request identity stays Host-local (not on `ene-api`). The exact
/// target text never rides this DTO: the seated Owner already typed it,
/// and GUI snapshots must not reconstruct it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PendingDeletionPreview {
    pub request_id: String,
    pub purpose: String,
}

/// Messages a seated control speaker may send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToHost {
    SeatHello,
    DeviceApprove {
        pending_id: String,
    },
    CredentialPut {
        provider: String,
        label: String,
        secret: RedactedSecret,
    },
    DeletionConfirm {
        request_id: String,
    },
    /// Seated read of staged Targeted Deletion request identities.
    PendingDeletions,
    /// Owner-initiated resume of a Held operation. The operation was
    /// already admitted; this is not a second destructive confirmation.
    DeletionResume {
        operation: String,
        sweep: u64,
    },
    SessionComplete {
        session_id: Uuid,
        nonce: String,
    },
    /// Session-less self-declaration. Always [`FromHost::DeniedByBoundary`].
    ConfirmedTrue,
}

/// Messages the Host sends on the control channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FromHost {
    SeatGranted,
    SeatOccupied,
    DeniedByBoundary,
    ConfirmationChallenge {
        session_id: Uuid,
        op: ControlOp,
        target: String,
        premise_generation: u64,
        nonce: String,
    },
    Outcome(ControlOutcome),
    Unavailable,
    PendingDeletions {
        requests: Vec<PendingDeletionPreview>,
    },
}

/// Non-secret completion facts. Pairing secrets for Host-local display use
/// [`RedactedSecret`] so `Debug` cannot leak them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlOutcome {
    DeviceApproved {
        pending_id: String,
        pairing_secret: RedactedSecret,
    },
    DeviceUnknown {
        pending_id: String,
    },
    CredentialStored {
        provider: String,
        label: String,
    },
    CredentialRefused {
        provider: String,
        label: String,
    },
    DeletionStarted {
        operation: String,
        sweep: u64,
    },
    DeletionAlreadyCoveredBy {
        operation: String,
        sweep: u64,
    },
    DeletionHeldByOperation {
        operation: String,
        sweep: u64,
    },
    DeletionNeedsClarification,
    DeletionMissing,
    DeletionResumed {
        operation: String,
        sweep: u64,
    },
    SessionCompleted {
        session_id: Uuid,
    },
}

#[cfg(test)]
mod tests {
    use super::{FromHost, RedactedSecret, ToHost};

    #[test]
    fn redacted_secret_debug_does_not_show_raw() {
        let secret = RedactedSecret::new("sk-live-secret");
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, "[redacted]");
        assert!(
            !rendered.contains("sk-live"),
            "Debug must not contain the raw secret"
        );
        let request = ToHost::CredentialPut {
            provider: String::from("openai"),
            label: String::from("main"),
            secret,
        };
        let debug = format!("{request:?}");
        assert!(
            !debug.contains("sk-live"),
            "ToHost Debug must redact the secret, got {debug}"
        );
    }

    #[test]
    fn control_frames_round_trip_json_without_debug_leak() {
        let request = ToHost::CredentialPut {
            provider: String::from("openai"),
            label: String::from("main"),
            secret: RedactedSecret::new("sk-live-secret"),
        };
        let json = serde_json::to_string(&request).expect("must serialize");
        let back: ToHost = serde_json::from_str(&json).expect("must deserialize");
        match back {
            ToHost::CredentialPut { secret, .. } => {
                assert_eq!(secret.expose(), "sk-live-secret");
            }
            other => panic!("expected CredentialPut, got {other:?}"),
        }
        let occupied = serde_json::to_string(&FromHost::SeatOccupied).expect("must serialize");
        let parsed: FromHost = serde_json::from_str(&occupied).expect("must deserialize");
        assert!(matches!(parsed, FromHost::SeatOccupied));
    }

    #[test]
    fn pending_deletion_preview_has_no_target_body_field() {
        let preview = super::PendingDeletionPreview {
            request_id: String::from("00000000-0000-0000-0000-000000000001"),
            purpose: String::from("privacy"),
        };
        let json = serde_json::to_string(&preview).expect("must serialize");
        assert!(
            !json.contains("exact"),
            "pending preview must not carry target text: {json}"
        );
        assert!(json.contains("privacy"));
    }
}
