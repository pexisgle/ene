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
    Started { operation: String, sweep: u64 },
    AlreadyCoveredBy { operation: String, sweep: u64 },
    HeldByOperation { operation: String, sweep: u64 },
    NeedsClarification,
    Missing,
    Resumed { operation: String, sweep: u64 },
    StaleSweep { operation: String, sweep: u64 },
    Completed { operation: String, sweep: u64 },
    Finalizing { operation: String, sweep: u64 },
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
    BackpressureHold,
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
    SessionComplete {
        session_id: Uuid,
        nonce: RedactedSecret,
    },
    SessionReject {
        session_id: Uuid,
        nonce: RedactedSecret,
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
