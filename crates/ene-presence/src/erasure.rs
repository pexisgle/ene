use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, LocalErasurePass,
    ParticipantCompletionFact, ParticipantOwnerRef, drive_local_erasure,
};

use crate::PresenceTechnicalError;

pub trait PresenceErasureRepository: Send + Sync {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<Output = Result<LocalErasurePass, PresenceTechnicalError>> + Send;
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
        Box::pin(drive_local_erasure(
            Self::OWNER,
            command,
            |condition, text| {
                let repository = Arc::clone(&self.repository);
                Box::pin(async move { repository.erase_target_text(condition, text).await })
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ene_preservation::{
        DeletionOperationId, DeletionSearchMaterial, DeletionSweepGeneration,
        MechanicalDeletionTarget, ParticipantErasureScope, ParticipantHoldClass,
        TargetedDeletionTarget,
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
        outcome: Mutex<LocalErasurePass>,
        calls: Mutex<usize>,
    }

    impl PresenceErasureRepository for FakeRepo {
        fn erase_target_text(
            &self,
            _condition: ErasureConditionRef,
            _target: &str,
        ) -> impl std::future::Future<Output = Result<LocalErasurePass, PresenceTechnicalError>> + Send
        {
            *self.calls.lock().unwrap() += 1;
            let outcome = *self.outcome.lock().unwrap();
            async move { Ok(outcome) }
        }
    }

    fn fake(outcome: LocalErasurePass) -> (Arc<FakeRepo>, PresenceErasureParticipant<FakeRepo>) {
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(outcome),
            calls: Mutex::new(0),
        });
        let participant = PresenceErasureParticipant::new(Arc::clone(&repo));
        (repo, participant)
    }

    #[tokio::test]
    async fn a_zero_remainder_pass_is_a_verified_fact() {
        let (repo, participant) = fake(LocalErasurePass::Applied {
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
        let (_repo, participant) = fake(LocalErasurePass::Applied {
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
        let (_repo, participant) = fake(LocalErasurePass::NotCurrent);
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
        let (repo, participant) = fake(LocalErasurePass::Applied {
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
