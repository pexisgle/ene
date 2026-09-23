//! Host-local control DTOs. Not on `ene-api`, not remote-capable.
//!
//! Two channels with different authority share this crate:
//!
//! - [`ToHost`] / [`FromHost`] speak the **requester listener**, a local
//!   endpoint any same-user process may dial. It carries non-secret requests
//!   and non-secret outcomes. It never issues a seat, never carries a secret,
//!   and never completes a [`FromConfirmation::ConfirmationChallenge`] session.
//! - [`ToConfirmation`] / [`FromConfirmation`] speak the **inherited
//!   confirmation channel** the Host hands to the GUI it spawned. Only this
//!   channel carries challenges, secret intake, and session completion.
//!
//! Secret fields use [`RedactedSecret`]: `Debug` never prints the raw value.
//! A completion is a seat-bound session id plus a freshness nonce; knowing a
//! nonce, declaring a PID, or opening the requester listener grants nothing.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub mod channel;

pub use channel::{
    CONFIRMATION_MODE_ENV, CONFIRMATION_MODE_STDIO, ChildHandles, GuiChannel, HostChannel,
};

/// Volatile secret carried only on the confirmation intake path.
///
/// Serialized for the Host-local frame; never shown by `Debug`. Drop
/// zeroizes the buffer. This type is not an `ene-api` payload.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
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

/// Targeted Deletion outcomes shared by both channels.
///
/// The requester learns the same non-secret lifecycle facts the confirming
/// GUI does; neither channel turns them into a completion claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionOutcome {
    Started { operation: String, sweep: u64 },
    AlreadyCoveredBy { operation: String, sweep: u64 },
    HeldByOperation { operation: String, sweep: u64 },
    NeedsClarification,
    Missing,
    Resumed { operation: String, sweep: u64 },
}

/// What a requester may send on the requester listener.
///
/// Requests only. `RequestCredentialPut` deliberately carries no secret: the
/// raw value is accepted from the inherited confirmation channel alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHost {
    /// Launcher request: show the official GUI. The Host starts its own child
    /// with the private confirmation channel; a second request converges on
    /// the live GUI instead of opening another seat.
    OpenDesktop,
    RequestDeviceApprove {
        pending_id: String,
    },
    RequestCredentialPut {
        provider: String,
        label: String,
    },
    RequestDeletionConfirm {
        request_id: String,
    },
    /// Read of staged Targeted Deletion request identities.
    PendingDeletions,
    /// Non-secret status of one accepted request, by the Host-issued id.
    RequestStatus {
        request_id: String,
    },
    /// Session-less self-declaration. Always [`FromHost::DeniedByBoundary`].
    ConfirmedTrue,
}

/// Lifecycle of one accepted requester request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestState {
    /// The Owner's confirmation surface is showing the challenge.
    AwaitingOwnerConfirmation,
    /// No live confirmation surface and none could be started: zero mutation.
    ConfirmationUnavailable,
    /// The Owner declined, or the session expired before completion.
    Rejected,
    /// The premise moved; a new request against the current state is needed.
    StalePremise,
    /// The owner boundary decided; `outcome` carries the non-secret facts.
    Applied { outcome: RequesterOutcome },
    /// The durable outcome could not be read. Never reported as success.
    OutcomeUnavailable,
}

/// Non-secret outcome facts a requester may observe.
///
/// The pairing secret is deliberately absent: it belongs to the confirmation
/// channel and the pairing Client's own provisioning path, never to a
/// requester's stdout or a business DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequesterOutcome {
    DeviceApproved {
        pending_id: String,
        device_id: String,
    },
    DeviceUnknown {
        pending_id: String,
    },
    CredentialStored {
        provider: String,
        label: String,
    },
    /// The value reached the OS store, but the approval sweep and the usable
    /// reference did not commit. Neither `CredentialStored` nor "nothing
    /// happened": recovery inspects the pending pair before another attempt.
    CredentialUncommitted {
        provider: String,
        label: String,
    },
    CredentialRefused {
        provider: String,
        label: String,
    },
    Deletion(DeletionOutcome),
}

/// Answers the requester listener may send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FromHost {
    /// A GUI is live (started now or already). The seat stays with the child
    /// the Host spawned.
    DesktopOpened,
    /// No GUI could be started. No confirmation surface exists.
    DesktopUnavailable,
    /// The request was accepted under this Host-issued id.
    RequestAccepted { request_id: String },
    RequestStatus {
        request_id: String,
        state: RequestState,
    },
    PendingDeletions {
        requests: Vec<PendingDeletionPreview>,
    },
    /// The request itself was refused by the boundary (never a confirmation).
    DeniedByBoundary,
    /// The Host cannot answer technically. Never a domain outcome.
    Unavailable,
}

/// What the Host-spawned GUI may send on its inherited confirmation channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToConfirmation {
    /// Owner-initiated resume of a Held Targeted Deletion operation. The
    /// operation was already admitted; this continues it and is not a second
    /// destructive confirmation.
    DeletionResume { operation: String, sweep: u64 },
    /// Secret intake for one live credential session. The raw value rides only
    /// this channel, bound to the session minted for the same operation.
    CredentialSecret {
        session_id: Uuid,
        nonce: String,
        provider: String,
        label: String,
        secret: RedactedSecret,
    },
    /// The Owner's direct confirmation on the challenge surface.
    SessionComplete { session_id: Uuid, nonce: String },
    /// The Owner declined on the challenge surface. Applies nothing.
    SessionReject { session_id: Uuid, nonce: String },
    /// Session-less self-declaration. Always [`FromConfirmation::DeniedByBoundary`].
    ConfirmedTrue,
}

/// Answers only the inherited confirmation channel may receive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FromConfirmation {
    /// One-shot challenge bound to the seat generation and the operation.
    ConfirmationChallenge {
        session_id: Uuid,
        op: ControlOp,
        target: String,
        premise_generation: u64,
        nonce: String,
    },
    /// Confirmation-channel outcome. Pairing completion carries only
    /// non-secret approval facts; provision uses the originating Client
    /// connection.
    Outcome(ControlOutcome),
    DeniedByBoundary,
    /// The confirmation surface cannot answer. Never a success.
    Unavailable,
}

/// Confirmation-channel completion facts.
///
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlOutcome {
    DeviceApproved {
        pending_id: String,
        device_id: String,
    },
    DeviceUnknown {
        pending_id: String,
    },
    CredentialStored {
        provider: String,
        label: String,
    },
    /// The Owner entered a value on the confirmation surface. It is staged,
    /// not stored: intake alone neither writes the OS store nor publishes a
    /// usable reference.
    CredentialStaged {
        provider: String,
        label: String,
    },
    /// The value reached the OS store, but the approval sweep and the usable
    /// reference did not commit. Neither a stored credential nor an untouched
    /// one: recovery inspects the pending pair before another attempt.
    CredentialUncommitted {
        provider: String,
        label: String,
    },
    CredentialRefused {
        provider: String,
        label: String,
    },
    Deletion(DeletionOutcome),
    /// The Owner declined the challenge on this surface.
    Rejected {
        session_id: Uuid,
    },
}

#[cfg(test)]
mod tests {
    use super::{ControlOp, FromConfirmation, FromHost, RedactedSecret, ToConfirmation, ToHost};

    #[test]
    fn redacted_secret_debug_does_not_show_raw() {
        let secret = RedactedSecret::new("sk-live-secret");
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, "[redacted]");
        assert!(
            !rendered.contains("sk-live"),
            "Debug must not contain the raw secret"
        );
        let request = ToConfirmation::CredentialSecret {
            session_id: uuid::Uuid::nil(),
            nonce: String::from("n"),
            provider: String::from("openai"),
            label: String::from("main"),
            secret,
        };
        let debug = format!("{request:?}");
        assert!(
            !debug.contains("sk-live"),
            "ToConfirmation Debug must redact the secret, got {debug}"
        );
    }

    #[test]
    fn the_requester_channel_cannot_express_a_completion() {
        // The requester's request/answer enums have no completion or
        // challenge frame at all: a requester cannot complete a session even
        // by sending raw JSON, because the listener decodes only these types.
        let requester = serde_json::to_string(&ToHost::ConfirmedTrue).expect("must serialize");
        assert!(requester.contains("ConfirmedTrue"));
        let challenge = FromConfirmation::ConfirmationChallenge {
            session_id: uuid::Uuid::nil(),
            op: ControlOp::DeviceApprove,
            target: String::from("pending-1"),
            premise_generation: 3,
            nonce: String::from("n"),
        };
        let as_requester_answer: Result<FromHost, _> =
            serde_json::from_str(&serde_json::to_string(&challenge).expect("must serialize"));
        assert!(
            as_requester_answer.is_err(),
            "a challenge frame must not decode as a requester answer"
        );
        let former_seat_hello: Result<ToHost, _> = serde_json::from_str(r#""SeatHello""#);
        assert!(
            former_seat_hello.is_err(),
            "the requester protocol must not retain an empty-seat acquisition frame"
        );
    }
}
