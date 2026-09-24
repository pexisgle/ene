use ene_credential::{CredentialSetRevision, ScrubbedText};
use ene_primitive::{RawId, WallClockWithTz};

use crate::agent::TaskAgentOutput;
use crate::delegation::DelegationId;
use crate::task::{TaskId, TaskRef, TaskRevision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskResultId(RawId);

impl TaskResultId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnadoptedResultCursor {
    pub recorded_at: WallClockWithTz,
    pub result: TaskResultId,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TaskResultScrubPremise {
    body: String,
    credential_set: CredentialSetRevision,
}

impl TaskResultScrubPremise {
    #[must_use]
    pub fn from_scrubbed(proof: ScrubbedText) -> Self {
        let credential_set = proof.credential_set();
        Self {
            body: proof.into_text(),
            credential_set,
        }
    }

    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    #[must_use]
    pub fn credential_set(&self) -> CredentialSetRevision {
        self.credential_set
    }
}

impl core::fmt::Debug for TaskResultScrubPremise {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskResultScrubPremise")
            .field("body", &"[redacted]")
            .field("credential_set", &self.credential_set)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentResultArrival {
    pub delegation: DelegationId,
    pub result: TaskResultId,
    pub body: TaskResultScrubPremise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResultArrivalOutcome {
    Recorded(TaskResultRecord),
    StaleCredentialSet { current: CredentialSetRevision },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResultAdoptionClaim {
    pub result: TaskResultId,
    pub attempt_refs: Vec<RawId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResultRecord {
    pub result: TaskResultId,
    pub task: TaskRef,
    pub delegation: DelegationId,
    pub body: TaskAgentOutput,
    pub attempt_refs: Vec<RawId>,
    pub adopted_revision: Option<TaskRevision>,
    pub recorded_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResultAcceptance {
    AdoptedAsCompletion(TaskRef),
    RecordedToOriginalOnly,
    WithheldByEffectFacts { attempts: Vec<RawId> },
    MissingResult { result: TaskResultId },
    MissingDelegation { delegation: DelegationId },
    MissingTask { task: TaskId },
}
