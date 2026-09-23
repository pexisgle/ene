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
//! The nonce and every secret field use [`RedactedSecret`]: `Debug` never
//! prints the raw value. A completion is a seat-bound session id plus a
//! freshness nonce; knowing a nonce, declaring a PID, or opening the requester
//! listener grants nothing.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub mod channel;

pub use channel::{
    CONFIRMATION_MODE_ENV, CONFIRMATION_MODE_STDIO, ChildHandles, GuiChannel, HostChannel,
};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlOp {
    DeviceApprove,
    CredentialPut,
    DeletionConfirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PendingDeletionPreview {
    pub request_id: String,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionOutcome {
    Started {
        operation: String,
        sweep: u64,
    },
    AlreadyCoveredBy {
        operation: String,
        sweep: u64,
    },
    HeldByOperation {
        operation: String,
        sweep: u64,
    },
    NeedsClarification,
    Missing,
    Resumed {
        operation: String,
        sweep: u64,
    },
    /// The named sweep is no longer current; a resume cannot apply.
    StaleSweep {
        operation: String,
        sweep: u64,
    },
    /// The operation already finished; nothing was resumed.
    Completed {
        operation: String,
        sweep: u64,
    },
    /// The operation is sealing its final boundary; a resume cannot apply.
    Finalizing {
        operation: String,
        sweep: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHost {
    OpenDesktop,
    RequestDeviceApprove { pending_id: String },
    RequestCredentialPut { provider: String, label: String },
    RequestDeletionConfirm { request_id: String },
    PendingDeletions,
    RequestStatus { request_id: String },
    ConfirmedTrue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestState {
    AwaitingOwnerConfirmation,
    ConfirmationUnavailable,
    Rejected,
    StalePremise,
    Applied { outcome: RequesterOutcome },
    OutcomeUnavailable,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FromHost {
    DesktopOpened,
    DesktopUnavailable,
    RequestAccepted {
        request_id: String,
    },
    RequestStatus {
        request_id: String,
        state: RequestState,
    },
    PendingDeletions {
        requests: Vec<PendingDeletionPreview>,
    },
    DeniedByBoundary,
    /// The requester queue is saturated; the request was not admitted. A hold,
    /// not a technical failure and not a boundary refusal: the same request may
    /// be retried once the queue drains.
    BackpressureHold,
    /// The Host cannot answer technically. Never a domain outcome.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToConfirmation {
    DeletionResume {
        operation: String,
        sweep: u64,
    },
    CredentialSecret {
        session_id: Uuid,
        nonce: RedactedSecret,
        provider: String,
        label: String,
        secret: RedactedSecret,
    },
    /// The Owner's direct confirmation on the challenge surface.
    SessionComplete {
        session_id: Uuid,
        nonce: RedactedSecret,
    },
    /// The Owner declined on the challenge surface. Applies nothing.
    SessionReject {
        session_id: Uuid,
        nonce: RedactedSecret,
    },
    /// Session-less self-declaration. Always [`FromConfirmation::DeniedByBoundary`].
    ConfirmedTrue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FromConfirmation {
    ConfirmationChallenge {
        session_id: Uuid,
        op: ControlOp,
        target: String,
        premise_generation: u64,
        nonce: RedactedSecret,
    },
    Outcome(ControlOutcome),
    DeniedByBoundary,
    Unavailable,
}

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
    CredentialStaged {
        provider: String,
        label: String,
    },
    CredentialUncommitted {
        provider: String,
        label: String,
    },
    CredentialRefused {
        provider: String,
        label: String,
    },
    Deletion(DeletionOutcome),
    Rejected {
        session_id: Uuid,
    },
}
