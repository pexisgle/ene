//! Credential publication: candidate versions, the activation commit, and the
//! immutable snapshot the running Host authenticates with (Stage 7 A1c).
//!
//! The OS protected store holds the value; SQLite holds only non-secret
//! references. They are not updated in one transaction, so the design fixes the
//! order and the states in between ([Credential publication]):
//!
//! 1. `Prepared` is durable before any OS write.
//! 2. The candidate is written to a **new** OS item, read back, and recorded as
//!    `Staged`. A write whose result is unknown is reconciled by reading that
//!    item; it is never repeated blindly.
//! 3. One SQLite transaction consumes the Owner's confirmation, sweeps the old
//!    and new values, points the active reference at the candidate version,
//!    advances the credential-set revision, and records `Activated` with its
//!    non-secret outcome.
//! 4. Only after that commit is the immutable snapshot published, and only
//!    then does the caller answer.
//!
//! [Credential publication]: ../../../../docs/design/concrete/credential-publication.md

use crate::CredentialTechnicalError;

/// Non-secret identity of one credential version.
///
/// Allocated by the store, never derived from the value: it is neither a hash
/// nor an encryption body nor an authentication capability. It names the OS
/// item so a retired version stays addressable for the leases that still need
/// to remove its value from delayed results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretVersionId(u64);

impl SecretVersionId {
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// The next version, or [`None`] when the counter is exhausted: a wrapped
    /// version would collide with a retired item.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// What kind of update one mutation performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationKind {
    /// A first registration, or a rotation of an existing value.
    Register,
    /// Invalidation of the active reference without a candidate version.
    Revoke,
}

impl MutationKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Revoke => "revoke",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "register" => Some(Self::Register),
            "revoke" => Some(Self::Revoke),
            _ => None,
        }
    }
}

/// Durable phase of one mutation.
///
/// The phases distinguish "the OS item exists" from "the value is active": a
/// crash between them must never read as success, and the cleanup of an
/// unused candidate must never read as an activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationPhase {
    /// Recorded before the candidate write; no OS item is assumed to exist.
    Prepared,
    /// The candidate is written and read back; not yet active.
    Staged,
    /// The active reference, the revision, and this phase committed together.
    Activated,
    /// The value is active but a retired version is not yet removed.
    CleanupPending,
    /// The mutation finished and its retired version is gone.
    Completed,
    /// The candidate was not adopted; it is not and never was active.
    Abandoned,
}

impl MutationPhase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Staged => "staged",
            Self::Activated => "activated",
            Self::CleanupPending => "cleanup-pending",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "prepared" => Some(Self::Prepared),
            "staged" => Some(Self::Staged),
            "activated" => Some(Self::Activated),
            "cleanup-pending" => Some(Self::CleanupPending),
            "completed" => Some(Self::Completed),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

/// Non-secret outcome of one mutation, kept independently of its phase so a
/// later cleanup or a later update cannot rewrite what the Owner decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    /// The value became active at this revision.
    Activated { revision: u64 },
    /// The active reference was invalidated at this revision.
    Revoked { revision: u64 },
    /// Another writer won the same expected revision; nothing changed here.
    Stale,
    /// The Owner declined, or the session expired before completion.
    Rejected,
    /// The OS store refused the value. Nothing is active.
    Refused,
    /// The result could not be determined. Never success, never "not run".
    Unknown,
}

impl MutationOutcome {
    /// True only for an outcome that names a committed revision.
    #[must_use]
    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Activated { .. } | Self::Revoked { .. })
    }
}

/// Durable record of one credential mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMutation {
    pub mutation_id: String,
    pub kind: MutationKind,
    pub provider: String,
    pub label: String,
    /// Revision the caller built its premise on, if it had one.
    pub expected_revision: Option<u64>,
    pub candidate_version: Option<SecretVersionId>,
    pub phase: MutationPhase,
    pub outcome: Option<MutationOutcome>,
    /// Revision the outcome committed at, when it committed.
    pub decided_revision: Option<u64>,
}

/// The active and retired versions of one credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveVersion {
    pub active: Option<SecretVersionId>,
    /// A retired version whose OS item is not yet removed.
    pub cleanup: Option<SecretVersionId>,
}

/// Result of the activation transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationOutcome {
    /// Committed: the active reference, the revision, and the phase moved
    /// together, and `retired` is the version whose item still needs removal.
    Activated {
        revision: u64,
        retired: Option<SecretVersionId>,
    },
    /// Another writer advanced the revision first; nothing changed here.
    Stale { current_revision: u64 },
    /// The mutation id is unknown to this store.
    Missing,
    /// The mutation is already decided; the stored outcome stands.
    AlreadyDecided(MutationOutcome),
}

/// Persistence boundary for credential publication.
///
/// Implementations own the non-secret records only: no method accepts or
/// returns secret material, and the sweep that removes retired values runs
/// inside the activation transaction with the bearer the caller already holds
/// in its request-builder scope.
#[expect(
    async_fn_in_trait,
    reason = "Stage 7 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialPublicationRepository: Send + Sync {
    /// Records one mutation and its candidate item as `Prepared` before any OS
    /// write. Revocation has no candidate.
    ///
    /// A repeated call with the same mutation id and the same content returns
    /// the stored record: a retry observes the original attempt instead of
    /// starting a second one.
    async fn begin_credential_mutation(
        &self,
        mutation_id: String,
        kind: MutationKind,
        provider: String,
        label: String,
        expected_revision: Option<u64>,
        candidate_version: Option<SecretVersionId>,
    ) -> Result<CredentialMutation, CredentialTechnicalError>;

    /// Marks the already-recorded candidate as accepted by the OS store.
    async fn mark_credential_staged(
        &self,
        mutation_id: &str,
        candidate: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError>;

    /// Commits one activation under a single transaction.
    ///
    /// The sweep of `candidate_bearer` and of any `retired_bearer`, the usable
    /// reference, the active version, the credential-set revision, and the
    /// `Activated` phase with its outcome all commit together, so a premise
    /// taken before the call is either covered by the sweep or refused by the
    /// revision.
    async fn activate_credential(
        &self,
        mutation_id: &str,
        candidate_bearer: &str,
        retired_bearer: Option<&str>,
    ) -> Result<ActivationOutcome, CredentialTechnicalError>;

    /// Records a decided outcome that committed nothing (stale, refused,
    /// rejected, unknown) and abandons the candidate.
    async fn record_credential_mutation_outcome(
        &self,
        mutation_id: &str,
        outcome: MutationOutcome,
    ) -> Result<(), CredentialTechnicalError>;

    /// Reads one mutation, for retry idempotence and restart recovery.
    async fn credential_mutation(
        &self,
        mutation_id: &str,
    ) -> Result<Option<CredentialMutation>, CredentialTechnicalError>;

    /// Reads the active and retired versions of one credential.
    async fn active_credential_version(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<ActiveVersion, CredentialTechnicalError>;

    /// Marks a retired version's item as removed. The value is already
    /// inactive; this only records that the cleanup finished.
    async fn mark_credential_cleaned(
        &self,
        provider: &str,
        label: &str,
        version: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError>;
}
