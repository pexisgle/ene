//! Credential registration approval: pending registration requests and the
//! repository boundary that surfaces them.

use ene_primitive::WallClockWithTz;

use crate::CredentialTechnicalError;

/// One requested-but-not-yet-approved credential registration.
///
/// `provider` and `label` name the requested credential exactly as the future
/// [`CredentialRef`](crate::CredentialRef) would; they carry no secret material, so derived
/// [`core::fmt::Debug`] is safe. `requested_at` records when the request was
/// recorded, for display and audit only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingCredentialApproval {
    pub provider: String,
    pub label: String,
    pub requested_at: WallClockWithTz,
}

/// Persistence boundary for the pending credential-registration set.
///
/// Registration intent only proposes; a pending pair becomes usable through
/// the atomic credential-approval write path owned by the store. This trait
/// exposes only the durable pending set, so an unknown approval target can be
/// retried with the exact `(provider, label)` value.
///
/// Revocation is explicitly deferred: there is no remove/revoke method, so
/// approvals only accumulate.
///
/// Blank-input contract: Host ingress validates that provider and label are
/// non-blank before calling. Implementations perform no validation themselves
/// beyond never yielding blank entries from `list_pending`; callers must not
/// rely on this method to report validation errors.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialApprovalRepository: Send + Sync {
    async fn list_pending(
        &self,
    ) -> Result<Vec<PendingCredentialApproval>, CredentialTechnicalError>;
}
