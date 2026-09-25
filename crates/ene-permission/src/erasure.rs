use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, LocalErasurePass,
    ParticipantCompletionFact, ParticipantOwnerRef, drive_local_erasure,
};

use crate::PermissionTechnicalError;

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
