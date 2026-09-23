use ene_primitive::WallClockWithTz;

use crate::CredentialTechnicalError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingCredentialApproval {
    pub provider: String,
    pub label: String,
    pub requested_at: WallClockWithTz,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialApprovalRepository: Send + Sync {
    async fn list_pending(
        &self,
    ) -> Result<Vec<PendingCredentialApproval>, CredentialTechnicalError>;
}
