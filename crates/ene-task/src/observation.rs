use ene_primitive::{RawId, WallClockWithTz};

use crate::delegation::DelegationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskAgentObservationId(RawId);

impl TaskAgentObservationId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    /// Mints one fresh occurrence identity.
    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// Premise of one durable observation occurrence.
///
/// The occurrence identity is caller-minted at observation time, consistent
/// with the other Task premises. `attempt` is the producing AU5 Action
/// attempt where one exists; a refusal observation has none. `observed` is the
/// body the execution is about to replay when the observation reproduced
/// workspace content (`read` bytes or a `list` listing): the repository
/// compares it against the canonical current erasure conditions inside the
/// same transaction as the row write and never persists it. A refusal
/// observation carries no body and no producing attempt.
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
