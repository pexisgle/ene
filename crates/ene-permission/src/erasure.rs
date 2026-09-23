//! Permission-owned Targeted Deletion participant (lifecycle §9-§10).
//!
//! The permission / constraint owner holds two target-bearing surfaces
//! (SO §4.19, targeted-deletion §4.2 row "権限判断・同意・利用上限の記録"):
//!
//! - the write-once management-intent journal
//!   ([`IntentOutcomeRepository`](crate::IntentOutcomeRepository)'s durable
//!   rows), whose `target` and `rationale_quote` can carry caller text; and
//! - the current consent record per capability, whose route fields
//!   (identity, provider, model, credential id) can name the target.
//!
//! Owner contract for erasure, in contrast to deleting permission rules:
//!
//! - A journal row is **redacted in place**, never deleted. Deleting a decided
//!   intent would reopen its identity, so a retried id could re-execute a
//!   decision the Owner already received; a redacted fingerprint instead
//!   answers a same-id body-carrying retry with a conflict (clarification).
//!   The row keeps its `intent_id`, kind, outcome, and host-rendered marks.
//! - A consent record that carries the target text is **invalidated**
//!   (deleted). Erasure never rewrites a route in place and never grants
//!   anything: the required current control outcome is "no consent", which
//!   fails closed until the Owner re-assigns. Erasing a rule is never
//!   interpreted as "allowed".
//! - Host-generated vocabulary (capability names, intent kinds, view marks,
//!   revisions, UUID identities) is not a copy of caller text and is never
//!   mutated: redacting a derived mark would corrupt canonical state for a
//!   coincidence.
//!
//! The implementation receives only the protected exact-text material of the
//! operation (lifecycle §9) and is idempotent: a repeated demand for the same
//! `(operation, sweep, participant)` finds nothing to erase and never
//! re-applies a semantic effect (§9.1). An erasure pass whose condition is no
//! longer the operation's current unfinished condition mutates nothing, so a
//! late duplicate from a superseded sweep can never erase text the Owner
//! provided after completion (§7).

use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::PermissionTechnicalError;

/// Result of one bounded local erasure pass over permission-owned state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionErasureOutcome {
    /// The condition is not the operation's current unfinished condition (a
    /// superseded sweep or a completed operation): nothing was read or
    /// mutated, and no verification is claimed for it.
    NotCurrent,
    /// The bounded pass ran: `erased` target-bearing values were redacted or
    /// invalidated, `remainder` target-bearing values remain.
    Applied { erased: u64, remainder: u64 },
}

/// Store port for the permission owner's local erasure.
///
/// The `ene-store` adapter implements this against the canonical tables; a
/// participant that cannot reach its store reports a hold instead of a
/// verification, never a fake success.
pub trait PermissionErasureRepository: Send + Sync {
    /// Runs one bounded local erasure pass for `condition`.
    ///
    /// The pass must be idempotent for the same `(condition, target)` and must
    /// not mutate anything when `condition` is not current.
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<PermissionErasureOutcome, PermissionTechnicalError>,
    > + Send;
}

/// The permission owner's [`ErasureParticipant`] implementation.
///
/// The Host composition registers one instance; this type never depends on a
/// concrete storage backend, so the same adapter serves the production store
/// and store fakes.
pub struct PermissionErasureParticipant<R> {
    repository: Arc<R>,
}

impl<R> PermissionErasureParticipant<R> {
    const OWNER: ParticipantOwnerRef = ParticipantOwnerRef::Permission;

    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }
}

impl<R: PermissionErasureRepository + 'static> ErasureParticipant
    for PermissionErasureParticipant<R>
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
            // A correlation-only demand carries no protected material: this
            // in-process owner cannot run its mechanical pass, so it holds the
            // sweep explicitly instead of claiming a verification it did not
            // perform (lifecycle §8.1/§9).
            let Some(target) = command.scope().target() else {
                return ParticipantCompletionFact::held(
                    condition,
                    Self::OWNER,
                    ParticipantHoldClass::Failed,
                    WallClockWithTz::now(),
                );
            };
            let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
            let text = material.expose_for_erasure();
            if text.trim().is_empty() {
                // The admission boundary refuses a blank mechanical target; a
                // blank demand is not a verifiable scope.
                return ParticipantCompletionFact::held(
                    condition,
                    Self::OWNER,
                    ParticipantHoldClass::Failed,
                    WallClockWithTz::now(),
                );
            }
            match self.repository.erase_target_text(condition, text).await {
                // Not-current work mutates nothing and cannot verify the
                // demanded condition; the durable record refuses it as stale
                // or completed, so the pass ends without a false completion.
                Ok(PermissionErasureOutcome::NotCurrent) => {
                    ParticipantCompletionFact::local_complete(
                        condition,
                        Self::OWNER,
                        0,
                        0,
                        WallClockWithTz::now(),
                    )
                }
                Ok(PermissionErasureOutcome::Applied {
                    erased,
                    remainder: 0,
                }) => ParticipantCompletionFact::verified(
                    condition,
                    Self::OWNER,
                    erased,
                    WallClockWithTz::now(),
                ),
                Ok(PermissionErasureOutcome::Applied { erased, remainder }) => {
                    ParticipantCompletionFact::more_work(
                        condition,
                        Self::OWNER,
                        erased,
                        remainder,
                        WallClockWithTz::now(),
                    )
                }
                // Storage failure is a hold, never a completion: the residual
                // target-bearing state is unproven, so the operation stays
                // retryable-incomplete.
                Err(_) => ParticipantCompletionFact::held(
                    condition,
                    Self::OWNER,
                    ParticipantHoldClass::Failed,
                    WallClockWithTz::now(),
                ),
            }
        })
    }
}
