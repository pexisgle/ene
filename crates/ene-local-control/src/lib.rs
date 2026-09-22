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

    #[must_use]
    pub fn into_inner(self) -> String {
        let mut owned = self;
        core::mem::take(&mut owned.0)
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
    Started { operation: String, sweep: u64 },
    AlreadyCoveredBy { operation: String, sweep: u64 },
    HeldByOperation { operation: String, sweep: u64 },
    NeedsClarification,
    Missing,
    Resumed { operation: String, sweep: u64 },
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
        nonce: String,
        provider: String,
        label: String,
        secret: RedactedSecret,
    },
    SessionComplete {
        session_id: Uuid,
        nonce: String,
    },
    SessionReject {
        session_id: Uuid,
        nonce: String,
    },
    ConfirmedTrue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FromConfirmation {
    ConfirmationChallenge {
        session_id: Uuid,
        op: ControlOp,
        target: String,
        premise_generation: u64,
        nonce: String,
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
