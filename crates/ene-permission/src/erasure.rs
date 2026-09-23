use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, LocalErasurePass,
    ParticipantCompletionFact, ParticipantOwnerRef, drive_local_erasure,
};

use crate::PermissionTechnicalError;

/// Store port for the permission owner's local erasure.
///
/// The `ene-store` adapter implements this against the canonical tables; a
/// participant that cannot reach its store reports a hold instead of a
/// verification, never a fake success.
pub trait PermissionErasureRepository: Send + Sync {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<Output = Result<LocalErasurePass, PermissionTechnicalError>> + Send;
}

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
        MechanicalDeletionTarget, ParticipantHoldClass, TargetedDeletionTarget,
    };
    use ene_primitive::RawId;

    fn condition() -> ErasureConditionRef {
        ErasureConditionRef {
            operation: DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(1),
        }
    }

    fn command(
        condition: ErasureConditionRef,
        scope: ene_preservation::ParticipantErasureScope,
    ) -> DemandLocalErasureCommand {
        DemandLocalErasureCommand::new(condition, ParticipantOwnerRef::Permission, scope)
    }

    fn local_scope(text: &str) -> ene_preservation::ParticipantErasureScope {
        ene_preservation::ParticipantErasureScope::local(TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: vec![],
        })
    }

    struct FakeRepo {
        outcome: Mutex<LocalErasurePass>,
        called_with: Mutex<Vec<String>>,
    }

    impl FakeRepo {
        fn new(outcome: LocalErasurePass) -> Self {
            Self {
                outcome: Mutex::new(outcome),
                called_with: Mutex::new(vec![]),
            }
        }
    }

    impl PermissionErasureRepository for FakeRepo {
        fn erase_target_text(
            &self,
            _condition: ErasureConditionRef,
            target: &str,
        ) -> impl std::future::Future<Output = Result<LocalErasurePass, PermissionTechnicalError>> + Send
        {
            let outcome = *self.outcome.lock().unwrap();
            self.called_with.lock().unwrap().push(target.to_owned());
            async move { Ok(outcome) }
        }
    }

    #[tokio::test]
    async fn a_zero_remainder_pass_is_a_verified_fact() {
        let repo = Arc::new(FakeRepo::new(LocalErasurePass::Applied {
            erased: 3,
            remainder: 0,
        }));
        let participant = PermissionErasureParticipant::new(Arc::clone(&repo));
        assert_eq!(participant.owner(), ParticipantOwnerRef::Permission);
        let condition = condition();
        let fact = participant
            .demand_local_erasure(command(condition, local_scope("private target")))
            .await;
        assert_eq!(fact.condition(), condition);
        assert_eq!(fact.participant(), ParticipantOwnerRef::Permission);
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Verified
        );
        assert_eq!((fact.erased_count(), fact.remainder_count()), (3, 0));
        assert_eq!(
            repo.called_with.lock().unwrap().as_slice(),
            ["private target".to_owned()]
        );
        assert!(!format!("{fact:?}").contains("private target"));
    }

    #[tokio::test]
    async fn remaining_matches_report_more_work() {
        let repo = Arc::new(FakeRepo::new(LocalErasurePass::Applied {
            erased: 1,
            remainder: 2,
        }));
        let participant = PermissionErasureParticipant::new(repo);
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("private target")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::MoreWork
        );
        assert_eq!((fact.erased_count(), fact.remainder_count()), (1, 2));
    }

    #[tokio::test]
    async fn a_not_current_pass_claims_no_verification() {
        let repo = Arc::new(FakeRepo::new(LocalErasurePass::NotCurrent));
        let participant = PermissionErasureParticipant::new(repo);
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("private target")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::LocalComplete
        );
        assert_eq!((fact.erased_count(), fact.remainder_count()), (0, 0));
    }

    #[tokio::test]
    async fn a_correlation_only_or_blank_demand_is_an_explicit_hold() {
        let repo = Arc::new(FakeRepo::new(LocalErasurePass::Applied {
            erased: 0,
            remainder: 0,
        }));
        let participant = PermissionErasureParticipant::new(Arc::clone(&repo));
        for scope in [
            ene_preservation::ParticipantErasureScope::correlation_only(),
            local_scope(" "),
        ] {
            let fact = participant
                .demand_local_erasure(command(condition(), scope))
                .await;
            assert_eq!(
                fact.status(),
                ene_preservation::ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
            );
            assert_eq!((fact.erased_count(), fact.remainder_count()), (0, 0));
        }
        assert!(repo.called_with.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_storage_failure_is_a_hold_not_a_completion() {
        struct FailingRepo;
        impl PermissionErasureRepository for FailingRepo {
            async fn erase_target_text(
                &self,
                _condition: ErasureConditionRef,
                _target: &str,
            ) -> Result<LocalErasurePass, PermissionTechnicalError> {
                Err(PermissionTechnicalError::StorageUnavailable {
                    reason: String::from("fixture"),
                })
            }
        }
        let participant = PermissionErasureParticipant::new(Arc::new(FailingRepo));
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("private target")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
        );
    }
}
