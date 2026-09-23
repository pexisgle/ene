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
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentObservationPremise {
    pub observation: TaskAgentObservationId,
    pub delegation: DelegationId,
    pub attempt: Option<RawId>,
    pub observed: Option<String>,
    pub observed_at: WallClockWithTz,
}
