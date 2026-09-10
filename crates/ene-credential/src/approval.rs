//! Credential registration approval: pending registration requests and the
//! repository boundary that records the owner's decision that a credential is
//! usable.

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
    /// Provider the requested credential belongs to.
    pub provider: String,
    /// Owner-chosen label distinguishing credentials of one provider.
    pub label: String,
    /// Wall-clock time with its creation offset recording when requested.
    pub requested_at: WallClockWithTz,
}

/// Persistence boundary for credential registration approvals.
///
/// Trust story: registration intent only proposes. A registration request
/// (for example, one arriving over the wire) calls `request_approval`, which
/// records a pending entry and never a usable credential. A Host-local
/// trusted inlet carrying explicit owner confirmation then calls
/// `approve_pending`, which flips the pending entry to usable. Availability
/// stays two-part: [`credential_availability`](crate::credential_availability) still reports ref-usable
/// (repository) AND bearer-present (store); callers additionally gate on
/// `is_approved`, and that gating lives in Host composition, NOT here. This
/// crate provides the approval fact; it never combines it with availability
/// itself.
///
/// Revocation is explicitly deferred: Stage 2 thin scope provides no
/// remove/revoke method, so approvals only accumulate.
///
/// Blank-input contract: Host ingress validates that provider and label are
/// non-blank before calling. Implementations perform no validation
/// themselves beyond treating blank input as absent: when `provider` or
/// `label` is empty or whitespace-only, `request_approval` records nothing
/// and returns `Ok(false)`, `approve_pending` returns `Ok(false)`,
/// `is_approved` returns `Ok(false)`, and `list_pending` never yields blank
/// entries. A blank pair can therefore never become usable here; callers must
/// not rely on these methods to report validation errors.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialApprovalRepository: Send + Sync {
    /// Records a credential registration approval request.
    ///
    /// Returns `Ok(true)` only when a new pending entry was recorded.
    /// Returns `Ok(false)` without touching stored entries when a pending
    /// entry already exists for `(provider, label)`, when the pair is already
    /// approved, or when either input is blank (empty or whitespace-only;
    /// Host ingress validates non-blank before calling, so this is a
    /// defensive backstop, never validation feedback).
    async fn request_approval(
        &self,
        provider: String,
        label: String,
    ) -> Result<bool, CredentialTechnicalError>;

    /// Approves the pending request for `(provider, label)`, marking it usable.
    ///
    /// On a known pending pair this moves the entry from pending to usable
    /// and returns `Ok(true)`. Re-approving an already-usable pair returns
    /// `Ok(true)` idempotently with no state change. An unknown pair yields
    /// `Ok(false)` — not an error; the caller maps that outcome to a
    /// clarification request. A blank `provider` or `label` is treated as
    /// absent and likewise yields `Ok(false)` without recording anything.
    ///
    /// Returns `bool` rather than `Option`, unlike
    /// [`DevicePairingRepository::approve_pending`](crate::DevicePairingRepository::approve_pending), because there is no
    /// minted record or one-time secret to hand back: the approval fact
    /// itself is the whole result.
    ///
    /// Approval records an Owner decision transported from a trusted inlet;
    /// the repository never decides whether approval is allowed, it records
    /// the decision it was given.
    async fn approve_pending(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError>;

    /// Reports whether `(provider, label)` is approved (usable).
    ///
    /// Returns `Ok(true)` only after approval; pending-only, unknown, and
    /// blank pairs all yield `Ok(false)`.
    async fn is_approved(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError>;

    /// Lists all currently pending credential approval requests.
    async fn list_pending(
        &self,
    ) -> Result<Vec<PendingCredentialApproval>, CredentialTechnicalError>;
}
