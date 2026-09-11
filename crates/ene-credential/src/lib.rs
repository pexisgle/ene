//! Credential registry contracts: non-secret refs, secret hygiene, and the
//! request-builder store pattern.
//!
//! [`CredentialRef`] is the only credential value that may leave this crate
//! freely: it names a credential without carrying any secret material; key
//! material lives solely in [`SecretValue`].
//!
//! Secrets enter only through the Host-local protected path (a future
//! behaviors-stage store): [`RegisterCredentialCommand`] deliberately carries
//! no secret field, so registration can never smuggle key material through
//! the registry. [`CredentialStore::with_bearer`] exposes the bearer only
//! inside a caller closure; the caller must build an owned request there and
//! send it after the closure returns.

mod approval;
mod auth_file;
mod pairing;
mod registration;
mod registry;
mod scrub;
mod secret;

#[cfg(test)]
mod tests;

use thiserror::Error;

pub use approval::{CredentialApprovalRepository, PendingCredentialApproval};
pub use auth_file::FileDeviceAuthStore;
pub use pairing::{
    DeviceId, DevicePairingRepository, DevicePairingStatus, DeviceRecord, PendingPairing,
    pairing_proof_hex, verify_pairing_proof,
};
pub use registration::{
    CredentialIntentRepository, RegistrationApply, RegistrationFingerprint, RegistrationState,
};
pub use registry::{
    CredentialAvailability, CredentialNotify, CredentialRef, CredentialRefError,
    CredentialRefRepository, CredentialSetRepository, RegisterCredentialCommand, RegisterOutcome,
    available_credential, credential_availability, register,
};
pub use scrub::{
    CredentialSetRevision, CredentialSetState, CredentialValuesDigest,
    CredentialValuesDigestBuilder, ScrubbedText, SecretScrubError, SecretScrubber,
};
pub use secret::{
    CredentialStore, ENV_API_KEY, EnvCredentialStore, MemoryCredentialStore, SecretValue,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialTechnicalError {
    #[error("credential storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause, without secret material.
        reason: String,
    },
}

/// Display marker replacing one registered credential value.
///
/// The Host and the store share this single token so redaction is
/// recognisable end to end without carrying any part of the value.
pub const REDACTED_CREDENTIAL: &str = "[credential]";
