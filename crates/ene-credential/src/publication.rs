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

    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
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
    Activated,
    CleanupPending,
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

impl MutationOutcome {
    #[must_use]
    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Activated { .. } | Self::Revoked { .. })
    }
}

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
    pub decided_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveVersion {
    pub active: Option<SecretVersionId>,
    pub cleanup: Option<SecretVersionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationOutcome {
    Activated {
        revision: u64,
        retired: Option<SecretVersionId>,
    },
    Stale {
        current_revision: u64,
    },
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

    async fn record_credential_mutation_outcome(
        &self,
        mutation_id: &str,
        outcome: MutationOutcome,
    ) -> Result<(), CredentialTechnicalError>;

    async fn credential_mutation(
        &self,
        mutation_id: &str,
    ) -> Result<Option<CredentialMutation>, CredentialTechnicalError>;

    async fn active_credential_version(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<ActiveVersion, CredentialTechnicalError>;

    async fn mark_credential_cleaned(
        &self,
        provider: &str,
        label: &str,
        version: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError>;
}
