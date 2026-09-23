//! Credential-owned Targeted Deletion participant (lifecycle §9-§10).
//!
//! The credential / secret owner's local surface is **local metadata and local
//! copies**, never the external service account (SO §4.21, targeted-deletion
//! §4.2 row "漏洩した認証情報（Credential）のコピー"):
//!
//! - the usable-ref registry (`credential_ref`, including the derived
//!   `provider:label` identity), the pending registration rows, and the device
//!   pairing rows (`paired_device` / `pairing_pending`), whose descriptors are
//!   caller-supplied display text; and
//! - the protected device-auth file's entries
//!   ([`FileDeviceAuthStore`](crate::FileDeviceAuthStore)), whose descriptor is
//!   the same display text and whose key can be the target itself.
//!
//! Boundary decisions that the design fixes and this participant keeps:
//!
//! - **No external revoke or rotation.** Targeted Deletion erases local
//!   metadata; it never invalidates, rotates, or re-issues a credential at the
//!   external provider. The registered provider bearer itself stays under the
//!   credential store's own validity management; the erasure does not read,
//!   render, or delete protected bearer values (K-C: values never leave the
//!   credential boundary, and a completion fact never carries one).
//! - **Fail closed.** A credential ref whose identity carries the target is
//!   deleted, so the pair is no longer usable until the Owner re-registers;
//!   deleting a ref advances the credential-set revision, because the usable
//!   set changed and stale scrub premises must be refused. A device entry
//!   whose descriptor or key carries the target is removed, so the device must
//!   pair again.
//! - Host-stamped times (`paired_at`, `requested_at`) and the revision counter
//!   are not copies of caller text and are never mutated.
//!
//! The implementation receives only the protected exact-text material of the
//! operation (lifecycle §9) and is idempotent: a repeated demand finds nothing
//! left to erase and never advances the revision twice (§9.1). A pass whose
//! condition is no longer the operation's current unfinished condition mutates
//! nothing — neither the tables nor the device-auth file — so a late duplicate
//! from a superseded sweep can never erase text the Owner provided after
//! completion (§7).

use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::{CredentialTechnicalError, FileDeviceAuthStore};

/// Result of one bounded local erasure pass over credential-owned metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialErasureOutcome {
    /// The condition is not the operation's current unfinished condition (a
    /// superseded sweep or a completed operation): nothing was read or
    /// mutated, and no verification is claimed for it.
    NotCurrent,
    /// The bounded pass ran: `erased` target-bearing metadata rows were
    /// removed, `remainder` target-bearing metadata rows remain.
    Applied { erased: u64, remainder: u64 },
}

/// Store port for the credential owner's local metadata erasure.
///
/// The `ene-store` adapter implements this against the canonical tables; the
/// protected device-auth file is handled by the participant itself, because it
/// is this crate's own custody boundary and must be gated by the same
/// current-condition decision.
pub trait CredentialErasureRepository: Send + Sync {
    /// Runs one bounded local metadata erasure pass for `condition`.
    ///
    /// The pass must be idempotent for the same `(condition, target)` and must
    /// not mutate anything when `condition` is not current.
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
    > + Send;

    /// Whether `condition` is the operation's current unfinished condition.
    ///
    /// The protected device-auth file cannot share the metadata transaction.
    /// The participant re-reads this predicate immediately before mutating
    /// the file so a completed or superseded condition cannot erase a fresh
    /// post-closure origin that landed after the metadata pass committed.
    fn condition_is_current(
        &self,
        condition: ErasureConditionRef,
    ) -> impl std::future::Future<Output = Result<bool, CredentialTechnicalError>> + Send;

    /// Test-only pause immediately before the protected file mutation.
    ///
    /// Production implementations return a ready future. The participant
    /// re-reads [`Self::condition_is_current`] after this point and before
    /// taking the file lock.
    fn before_device_auth_file_erase(&self) -> impl std::future::Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// The credential owner's [`ErasureParticipant`] implementation.
pub struct CredentialErasureParticipant<R> {
    repository: Arc<R>,
    device_auth: Arc<FileDeviceAuthStore>,
}

impl<R> CredentialErasureParticipant<R> {
    const OWNER: ParticipantOwnerRef = ParticipantOwnerRef::Credential;

    #[must_use]
    pub fn new(repository: Arc<R>, device_auth: Arc<FileDeviceAuthStore>) -> Self {
        Self {
            repository,
            device_auth,
        }
    }
}

impl<R: CredentialErasureRepository + 'static> ErasureParticipant
    for CredentialErasureParticipant<R>
{
    fn owner(&self) -> ParticipantOwnerRef {
        Self::OWNER
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        Box::pin(async move {
            let condition = command.condition();
            let held = |reason: ParticipantHoldClass| {
                ParticipantCompletionFact::held(
                    condition,
                    Self::OWNER,
                    reason,
                    WallClockWithTz::now(),
                )
            };
            // A correlation-only demand carries no protected material: this
            // in-process owner cannot run its mechanical pass, so it holds the
            // sweep explicitly instead of claiming a verification (lifecycle
            // §8.1/§9).
            let Some(target) = command.scope().target() else {
                return held(ParticipantHoldClass::Failed);
            };
            let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
            let text = material.expose_for_erasure();
            if text.trim().is_empty() {
                return held(ParticipantHoldClass::Failed);
            }
            match self.repository.erase_target_text(condition, text).await {
                // Not-current work mutates nothing, including the protected
                // file: the durable record refuses the fact as stale or
                // completed, so the pass ends without a false completion.
                Ok(CredentialErasureOutcome::NotCurrent) => {
                    ParticipantCompletionFact::local_complete(
                        condition,
                        Self::OWNER,
                        0,
                        0,
                        WallClockWithTz::now(),
                    )
                }
                Ok(CredentialErasureOutcome::Applied { erased, remainder }) => {
                    // The metadata transaction already committed under a
                    // then-current condition. The file is a different writer:
                    // park (tests) then re-read canonical currentness
                    // immediately before any file mutation so a completed
                    // operation cannot delete a fresh post-closure device
                    // entry of the same string.
                    self.repository.before_device_auth_file_erase().await;
                    let still_current = self
                        .repository
                        .condition_is_current(condition)
                        .await
                        .unwrap_or(false);
                    if !still_current {
                        return ParticipantCompletionFact::local_complete(
                            condition,
                            Self::OWNER,
                            0,
                            0,
                            WallClockWithTz::now(),
                        );
                    }
                    // The protected device-auth file is part of the same
                    // credential-owned local surface and runs only after the
                    // database pass proved the condition current.
                    let file_erased = match self.device_auth.erase_target_text(text) {
                        Ok(erased) => erased,
                        Err(_) => return held(ParticipantHoldClass::Failed),
                    };
                    let file_remainder = match self.device_auth.count_target_text(text) {
                        Ok(remainder) => remainder,
                        Err(_) => return held(ParticipantHoldClass::Failed),
                    };
                    let erased = erased.saturating_add(file_erased);
                    let remainder = remainder.saturating_add(file_remainder);
                    if remainder == 0 {
                        ParticipantCompletionFact::verified(
                            condition,
                            Self::OWNER,
                            erased,
                            WallClockWithTz::now(),
                        )
                    } else {
                        ParticipantCompletionFact::more_work(
                            condition,
                            Self::OWNER,
                            erased,
                            remainder,
                            WallClockWithTz::now(),
                        )
                    }
                }
                // Storage failure is a hold, never a completion: the residual
                // metadata is unproven, so the operation stays
                // retryable-incomplete.
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}
