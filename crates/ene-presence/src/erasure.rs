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
