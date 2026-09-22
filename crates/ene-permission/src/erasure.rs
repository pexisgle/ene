use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::PermissionTechnicalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionErasureOutcome {
    NotCurrent,
    Applied { erased: u64, remainder: u64 },
}

pub trait PermissionErasureRepository: Send + Sync {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<PermissionErasureOutcome, PermissionTechnicalError>,
    > + Send;
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
        TargetedDeletionTarget,
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
        outcome: Mutex<PermissionErasureOutcome>,
        called_with: Mutex<Vec<String>>,
    }

    impl FakeRepo {
        fn new(outcome: PermissionErasureOutcome) -> Self {
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
        ) -> impl std::future::Future<
            Output = Result<PermissionErasureOutcome, PermissionTechnicalError>,
        > + Send {
            let outcome = *self.outcome.lock().unwrap();
            self.called_with.lock().unwrap().push(target.to_owned());
            async move { Ok(outcome) }
        }
    }

    #[tokio::test]
    async fn a_zero_remainder_pass_is_a_verified_fact() {
        let repo = Arc::new(FakeRepo::new(PermissionErasureOutcome::Applied {
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
        let repo = Arc::new(FakeRepo::new(PermissionErasureOutcome::Applied {
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
        let repo = Arc::new(FakeRepo::new(PermissionErasureOutcome::NotCurrent));
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
        let repo = Arc::new(FakeRepo::new(PermissionErasureOutcome::Applied {
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
            ) -> Result<PermissionErasureOutcome, PermissionTechnicalError> {
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
