//! Credential registry contracts: non-secret refs, secret hygiene, and the
//! request-builder store pattern.
//!
//! [`CredentialRef`] is the only credential value that may leave this crate
//! freely: it names a credential without carrying any secret material; key
//! material lives solely in the crate-private `SecretValue`.
//!
//! Secrets enter only through the Host-local protected path: the
//! registration intent carries no secret field, so registration can never
//! smuggle key material through the registry. [`CredentialStore::with_bearer`]
//! exposes the bearer only inside a caller closure; the caller must build an
//! owned request there and send it after the closure returns.

mod approval;
mod auth_file;
mod erasure;
mod os_store;
mod pairing;
mod publication;
mod registration;
mod registry;
mod scrub;
mod secret;

#[cfg(test)]
mod tests;

use thiserror::Error;

pub use approval::{CredentialApprovalRepository, PendingCredentialApproval};
pub use auth_file::FileDeviceAuthStore;
pub use erasure::{
    CredentialErasureOutcome, CredentialErasureParticipant, CredentialErasureRepository,
};
pub use os_store::{DEFAULT_NAMESPACE, OsCredentialStore, service_name};
pub use pairing::{
    DeviceId, DevicePairingRepository, DeviceRecord, PairingSecretMaterial, PendingPairing,
    pairing_proof_hex, verify_pairing_proof,
};
pub use publication::{
    ActivationOutcome, ActiveVersion, CredentialMutation, CredentialPublicationRepository,
    MutationKind, MutationOutcome, MutationPhase, SecretVersionId, UncommittedMutationOutcome,
};
pub use registration::{
    CredentialIntentRepository, RegistrationApply, RegistrationFingerprint, RegistrationState,
};
pub use registry::{
    CredentialRef, CredentialRefError, CredentialRefRepository, CredentialSetRepository,
    available_credential,
};
pub use scrub::{
    CredentialScrubber, CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber,
};
pub use secret::{
    CredentialStore, ENV_API_KEY, EnvCredentialStore, MemoryCredentialStore, MemoryVersionedStore,
    PreparedCredentialSnapshot, VersionedCredentialStore,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialTechnicalError {
    #[error("credential storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

pub const REDACTED_CREDENTIAL: &str = "[credential]";
