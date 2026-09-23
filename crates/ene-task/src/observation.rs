use ene_primitive::{RawId, WallClockWithTz};

use crate::delegation::DelegationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskAgentObservationId(RawId);

impl TaskAgentObservationId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TaskAgentObservationPremise {
    pub observation: TaskAgentObservationId,
    pub delegation: DelegationId,
    pub attempt: Option<RawId>,
    pub observed: Option<String>,
    pub observed_at: WallClockWithTz,
}

impl core::fmt::Debug for TaskAgentObservationPremise {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskAgentObservationPremise")
            .field("observation", &self.observation)
            .field("delegation", &self.delegation)
            .field("attempt", &self.attempt)
            .field("observed", &"[redacted]")
            .field("observed_at", &self.observed_at)
            .finish()
    }
}
