use crate::CredentialTechnicalError;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationKind {
    Register,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationPhase {
    Prepared,
    Staged,
    /// The active reference, the revision, and this phase committed together,
    /// and the mutation retired no version that still needs removal.
    Activated,
    /// The reference and revision committed, but at least one version this
    /// mutation retired is not yet removed from the OS store.
    CleanupPending,
    /// The mutation finished and every version it retired is gone.
    Completed,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    Activated { revision: u64 },
    Revoked { revision: u64 },
    Stale,
    Rejected,
    Refused,
    Unknown,
}

/// Outcome of a mutation that committed nothing to the active reference or
/// revision.
///
/// Kept narrower than [`MutationOutcome`] so a caller cannot record a commit
/// (`Activated`/`Revoked`) through the non-committing journal path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncommittedMutationOutcome {
    /// Another writer won the same expected revision; nothing changed here.
    Stale,
    /// The Owner declined, or the session expired before completion.
    Rejected,
    /// The OS store refused the value. Nothing is active.
    Refused,
    /// The result could not be determined. Never success, never "not run".
    Unknown,
}

/// Durable record of one credential mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMutation {
    pub mutation_id: String,
    pub kind: MutationKind,
    pub provider: String,
    pub label: String,
    pub expected_revision: Option<u64>,
    pub candidate_version: Option<SecretVersionId>,
    pub phase: MutationPhase,
    pub outcome: Option<MutationOutcome>,
}

/// One retired version whose OS item removal is not yet confirmed.
///
/// The durable record is a set, not a single slot: every activation/rotation
/// or revocation enqueues the version it replaced in the same transaction
/// that moved the reference, and a later update never overwrites an earlier
/// pending retirement. The cleanup pass drains the set in bounded batches,
/// oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredCredentialVersion {
    pub provider: String,
    pub label: String,
    /// The retired version whose OS item still needs removal.
    pub version: SecretVersionId,
    /// The mutation that retired the version. Its phase reaches `Completed`
    /// only after this row's removal is recorded.
    pub mutation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationOutcome {
    /// Committed: the active reference, the revision, and the phase moved
    /// together, and `retired` is the version this commit retired and
    /// enqueued for cleanup (or `None` when nothing was active). The caller
    /// may remove the item immediately, but the durable retired row is the
    /// record a later pass retries; a failed immediate removal never changes
    /// this outcome.
    Activated {
        revision: u64,
        retired: Option<SecretVersionId>,
    },
    /// Another writer advanced the revision first; nothing changed here.
    Stale {
        current_revision: u64,
    },
    /// The store cannot resolve the id to an activatable, undecided mutation
    /// of the required kind and phase: unknown id, no candidate version, or a
    /// phase/kind this operation cannot decide.
    Missing,
    AlreadyDecided(MutationOutcome),
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 7 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialPublicationRepository: Send + Sync {
    async fn begin_credential_mutation(
        &self,
        mutation_id: String,
        kind: MutationKind,
        provider: String,
        label: String,
        expected_revision: Option<u64>,
        candidate_version: Option<SecretVersionId>,
    ) -> Result<CredentialMutation, CredentialTechnicalError>;

    async fn mark_credential_staged(
        &self,
        mutation_id: &str,
        candidate: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError>;

    async fn activate_credential(
        &self,
        mutation_id: &str,
        candidate_bearer: &str,
        retired_bearer: Option<&str>,
    ) -> Result<ActivationOutcome, CredentialTechnicalError>;

    /// Commits one revocation under a single transaction.
    ///
    /// The mutation must be an undecided [`MutationKind::Revoke`]. The sweep of
    /// `retired_bearer`, the cleared active reference (with the retired version
    /// recorded for cleanup), the credential-set revision, and the `Revoked`
    /// outcome commit together, so a premise taken before the call is either
    /// covered by the sweep or refused by the revision.
    async fn revoke_credential(
        &self,
        mutation_id: &str,
        retired_bearer: Option<&str>,
    ) -> Result<ActivationOutcome, CredentialTechnicalError>;

    /// Records a decided outcome that committed nothing to the active
    /// reference or revision (stale, refused, rejected, unknown) and abandons
    /// the candidate.
    async fn record_credential_mutation_outcome(
        &self,
        mutation_id: &str,
        outcome: UncommittedMutationOutcome,
    ) -> Result<(), CredentialTechnicalError>;

    async fn credential_mutation(
        &self,
        mutation_id: &str,
    ) -> Result<Option<CredentialMutation>, CredentialTechnicalError>;

    /// Reads the active version of one credential, if any.
    ///
    /// The retired set is read separately: a credential can have no active
    /// version and still have retired versions whose items await removal.
    async fn active_credential_version(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<SecretVersionId>, CredentialTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::{MutationKind, MutationPhase};

    #[test]
    fn phases_and_kinds_round_trip_their_durable_text() {
        for phase in [
            MutationPhase::Prepared,
            MutationPhase::Staged,
            MutationPhase::Activated,
            MutationPhase::CleanupPending,
            MutationPhase::Completed,
            MutationPhase::Abandoned,
        ] {
            assert_eq!(MutationPhase::parse(phase.as_str()), Some(phase));
        }
        for kind in [MutationKind::Register, MutationKind::Revoke] {
            assert_eq!(MutationKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(MutationPhase::parse("invented"), None);
        assert_eq!(MutationKind::parse("invented"), None);
    }
}
