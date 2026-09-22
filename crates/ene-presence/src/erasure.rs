use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::PresenceTechnicalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceErasureOutcome {
    NotCurrent,
    Applied { erased: u64, remainder: u64 },
}

pub trait PresenceErasureRepository: Send + Sync {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<Output = Result<PresenceErasureOutcome, PresenceTechnicalError>> + Send;
}

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
        ParticipantErasureScope::local(TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: vec![],
        })
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
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
        );
        assert_eq!(*repo.calls.lock().unwrap(), 0);
    }
}
