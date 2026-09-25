use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::{CredentialTechnicalError, FileDeviceAuthStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialErasureOutcome {
    NotCurrent,
    Applied { erased: u64, remainder: u64 },
}

pub trait CredentialErasureRepository: Send + Sync {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
    > + Send;

    fn condition_is_current(
        &self,
        condition: ErasureConditionRef,
    ) -> impl std::future::Future<Output = Result<bool, CredentialTechnicalError>> + Send;

    fn before_device_auth_file_erase(&self) -> impl std::future::Future<Output = ()> + Send {
        std::future::ready(())
    }
}

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
            let Some(target) = command.scope().target() else {
                return held(ParticipantHoldClass::Failed);
            };
            let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
            let text = material.expose_for_erasure();
            if text.trim().is_empty() {
                return held(ParticipantHoldClass::Failed);
            }
            match self.repository.erase_target_text(condition, text).await {
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
                    self.repository.before_device_auth_file_erase().await;
                    match self.repository.condition_is_current(condition).await {
                        Ok(true) => {}
                        Ok(false) => {
                            return ParticipantCompletionFact::local_complete(
                                condition,
                                Self::OWNER,
                                0,
                                0,
                                WallClockWithTz::now(),
                            );
                        }
                        Err(_) => return held(ParticipantHoldClass::Failed),
                    }
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
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::DeviceId;
    use ene_preservation::{
        DeletionOperationId, DeletionSearchMaterial, DeletionSweepGeneration,
        ParticipantErasureScope, TargetedDeletionTarget,
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
        scope: ParticipantErasureScope,
    ) -> DemandLocalErasureCommand {
        DemandLocalErasureCommand::new(condition, ParticipantOwnerRef::Credential, scope)
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
        outcome: Mutex<CredentialErasureOutcome>,
    }

    impl CredentialErasureRepository for FakeRepo {
        fn erase_target_text(
            &self,
            _condition: ErasureConditionRef,
            _target: &str,
        ) -> impl std::future::Future<
            Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
        > + Send {
            let outcome = *self.outcome.lock().unwrap();
            async move { Ok(outcome) }
        }

        fn condition_is_current(
            &self,
            _condition: ErasureConditionRef,
        ) -> impl std::future::Future<Output = Result<bool, CredentialTechnicalError>> + Send
        {
            let current = *self.outcome.lock().unwrap() != CredentialErasureOutcome::NotCurrent;
            async move { Ok(current) }
        }
    }

    fn device_file() -> (tempfile::TempDir, FileDeviceAuthStore, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-auth.json");
        let store = FileDeviceAuthStore::open(&path).unwrap();
        (dir, store, path)
    }

    #[tokio::test]
    async fn verified_not_current_and_correlation_passes_are_distinct() {
        let (_verified_dir, file, _path) = device_file();
        let device = DeviceId(RawId::new());
        file.save_secret(&device, "phone target-label", "device-secret")
            .unwrap();
        let other = DeviceId(RawId::new());
        file.save_secret(&other, "laptop", "other-secret").unwrap();
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(CredentialErasureOutcome::Applied {
                erased: 2,
                remainder: 0,
            }),
        });
        let participant = CredentialErasureParticipant::new(repo, Arc::new(file.clone()));
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("target-label")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Verified
        );
        assert_eq!(fact.erased_count(), 3);
        assert!(file.load_secret(&device).unwrap().is_none());
        assert!(file.load_secret(&other).unwrap().is_some());
        assert!(!format!("{fact:?}").contains("target-label"));

        let (_not_current_dir, file, _path) = device_file();
        let device = DeviceId(RawId::new());
        file.save_secret(&device, "phone target-label", "device-secret")
            .unwrap();
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(CredentialErasureOutcome::NotCurrent),
        });
        let participant = CredentialErasureParticipant::new(repo, Arc::new(file.clone()));
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("target-label")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::LocalComplete
        );
        assert!(file.load_secret(&device).unwrap().is_some());

        let (_correlation_dir, file, _path) = device_file();
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(CredentialErasureOutcome::Applied {
                erased: 0,
                remainder: 0,
            }),
        });
        let participant = CredentialErasureParticipant::new(repo, Arc::new(file));
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
    }

    #[tokio::test]
    async fn the_protected_file_erasure_is_idempotent_and_scoped() {
        let (_dir, file, _path) = device_file();
        let device = DeviceId(RawId::new());
        file.save_secret(&device, "phone target-label", "device-secret")
            .unwrap();
        assert_eq!(file.erase_target_text("target-label").unwrap(), 1);
        assert_eq!(
            file.erase_target_text("target-label").unwrap(),
            0,
            "a duplicate sweep finds nothing left to erase"
        );
        assert_eq!(file.count_target_text("target-label").unwrap(), 0);
        assert!(file.load_secret(&device).unwrap().is_none());
    }
}
