//! Presence-owned Targeted Deletion participant (lifecycle §9-§10).
//!
//! Presence attribution is a **non-text owner** (SO §4.15): its durable state
//! is opaque identity references (`companion_id`, `active_client`,
//! `last_client`, `recovery_destination`), the closed state/reason vocabulary,
//! generation counters, and host-stamped times. Only the identity references
//! can name a deletion target; the vocabulary, counters, and clocks are
//! derived state that may coincide with a target string but are not copies of
//! it, and mutating them would corrupt unrelated presence meaning.
//!
//! Owner contract for erasure:
//!
//! - A reference that names the target is erased, never redacted in place: an
//!   attribution whose companion identity is the target is deleted; an
//!   attribution whose active client is the target is stopped with the client
//!   cleared (a stopped companion is never moved, summoned, or recovered, so
//!   the erased client can never be re-crowned); a relocation hint clears the
//!   target-bearing reference; a transition-log row whose companion identity
//!   is the target is deleted.
//! - Unrelated presence state is left byte-identical: another companion's
//!   attribution, hint, and log stay untouched.
//! - When no presence-owned value contains the target, the bounded
//!   verification proves that absence and reports a `Verified` completion
//!   fact without inventing a data copy to erase.
//!
//! The implementation receives only the protected exact-text material of the
//! operation (lifecycle §9) and is idempotent: a repeated demand finds nothing
//! left to erase (§9.1). A pass whose condition is no longer the operation's
//! current unfinished condition mutates nothing, so a late duplicate from a
//! superseded sweep can never erase state the Owner re-created after
//! completion (§7).

use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::PresenceTechnicalError;

/// Result of one bounded local erasure pass over presence-owned state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceErasureOutcome {
    /// The condition is not the operation's current unfinished condition (a
    /// superseded sweep or a completed operation): nothing was read or
    /// mutated, and no verification is claimed for it.
    NotCurrent,
    /// The bounded pass ran: `erased` target-bearing values were removed or
    /// cleared, `remainder` target-bearing identity references remain.
    Applied { erased: u64, remainder: u64 },
}

/// Store port for the presence owner's local erasure.
pub trait PresenceErasureRepository: Send + Sync {
    /// Runs one bounded local erasure pass for `condition`.
    ///
    /// The pass must be idempotent for the same `(condition, target)` and must
    /// not mutate anything when `condition` is not current.
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<Output = Result<PresenceErasureOutcome, PresenceTechnicalError>> + Send;
}

/// The presence owner's [`ErasureParticipant`] implementation.
pub struct PresenceErasureParticipant<R> {
    repository: Arc<R>,
}

impl<R> PresenceErasureParticipant<R> {
    const OWNER: ParticipantOwnerRef = ParticipantOwnerRef::Presence;

    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }
}

impl<R: PresenceErasureRepository + 'static> ErasureParticipant for PresenceErasureParticipant<R> {
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
            // in-process owner cannot run its mechanical check, so it holds
            // the sweep explicitly instead of claiming a verification
            // (lifecycle §8.1/§9).
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
                Ok(PresenceErasureOutcome::NotCurrent) => {
                    ParticipantCompletionFact::local_complete(
                        condition,
                        Self::OWNER,
                        0,
                        0,
                        WallClockWithTz::now(),
                    )
                }
                Ok(PresenceErasureOutcome::Applied {
                    erased,
                    remainder: 0,
                }) => ParticipantCompletionFact::verified(
                    condition,
                    Self::OWNER,
                    erased,
                    WallClockWithTz::now(),
                ),
                Ok(PresenceErasureOutcome::Applied { erased, remainder }) => {
                    ParticipantCompletionFact::more_work(
                        condition,
                        Self::OWNER,
                        erased,
                        remainder,
                        WallClockWithTz::now(),
                    )
                }
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ene_preservation::{
        DeletionOperationId, DeletionSearchMaterial, DeletionSweepGeneration,
        ParticipantErasureScope, TargetedDeletionTarget,
    };
    use ene_primitive::RawId;

    fn condition() -> ErasureConditionRef {
        ErasureConditionRef {
            operation: DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(2),
        }
    }

    fn command(
        condition: ErasureConditionRef,
        scope: ParticipantErasureScope,
    ) -> DemandLocalErasureCommand {
        DemandLocalErasureCommand::new(condition, ParticipantOwnerRef::Presence, scope)
    }

    fn local_scope(text: &str) -> ParticipantErasureScope {
        ParticipantErasureScope::local(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    text.to_owned(),
                )),
                semantic_hints: vec![],
            },
            vec![],
        )
    }

    struct FakeRepo {
        outcome: Mutex<PresenceErasureOutcome>,
        calls: Mutex<usize>,
    }

    impl PresenceErasureRepository for FakeRepo {
        fn erase_target_text(
            &self,
            _condition: ErasureConditionRef,
            _target: &str,
        ) -> impl std::future::Future<
            Output = Result<PresenceErasureOutcome, PresenceTechnicalError>,
        > + Send {
            *self.calls.lock().unwrap() += 1;
            let outcome = *self.outcome.lock().unwrap();
            async move { Ok(outcome) }
        }
    }

    fn fake(
        outcome: PresenceErasureOutcome,
    ) -> (Arc<FakeRepo>, PresenceErasureParticipant<FakeRepo>) {
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(outcome),
            calls: Mutex::new(0),
        });
        let participant = PresenceErasureParticipant::new(Arc::clone(&repo));
        (repo, participant)
    }

    #[tokio::test]
    async fn a_zero_remainder_pass_is_a_verified_fact() {
        let (repo, participant) = fake(PresenceErasureOutcome::Applied {
            erased: 4,
            remainder: 0,
        });
        assert_eq!(participant.owner(), ParticipantOwnerRef::Presence);
        let condition = condition();
        let fact = participant
            .demand_local_erasure(command(condition, local_scope("client-target")))
            .await;
        assert_eq!(fact.condition(), condition);
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Verified
        );
        assert_eq!((fact.erased_count(), fact.remainder_count()), (4, 0));
        assert_eq!(*repo.calls.lock().unwrap(), 1);
        assert!(!format!("{fact:?}").contains("client-target"));
    }

    #[tokio::test]
    async fn a_remaining_reference_reports_more_work() {
        let (_repo, participant) = fake(PresenceErasureOutcome::Applied {
            erased: 0,
            remainder: 1,
        });
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("client-target")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::MoreWork
        );
        assert_eq!((fact.erased_count(), fact.remainder_count()), (0, 1));
    }

    #[tokio::test]
    async fn a_not_current_pass_claims_no_verification() {
        let (_repo, participant) = fake(PresenceErasureOutcome::NotCurrent);
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("client-target")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::LocalComplete
        );
    }

    #[tokio::test]
    async fn a_correlation_only_demand_is_an_explicit_hold() {
        let (repo, participant) = fake(PresenceErasureOutcome::Applied {
            erased: 0,
            remainder: 0,
        });
        let fact = participant
            .demand_local_erasure(command(
                condition(),
                ParticipantErasureScope::correlation_only(vec![RawId::new()]),
            ))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
        );
        assert_eq!(*repo.calls.lock().unwrap(), 0);
    }
}
