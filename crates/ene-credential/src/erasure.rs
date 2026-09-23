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
