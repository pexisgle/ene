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
    MutationKind, MutationOutcome, MutationPhase, SecretVersionId,
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
    PreparedCredentialSnapshot, SecretValue, VersionedCredentialStore,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialTechnicalError {
    #[error("credential storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

pub const REDACTED_CREDENTIAL: &str = "[credential]";
