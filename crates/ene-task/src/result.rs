//! Final Task result arrival, execution seal, and adoption (AU15a / AU15b).
//!
//! A final Task result is recorded at an explicit finalization boundary (the
//! Task Agent execution submits a final answer) before it is visible to any
//! caller: the `task_result` row itself is the execution seal, so one
//! delegation has at most one final result and no separate seal flag exists.
//! Provider output from a single inference turn is **not** a final result; a
//! tool loop may still turn it into an Action request, so the finalization
//! decision stays an explicit caller boundary.
//!
//! Adoption is separate from arrival. The `attempt_refs` a caller supplies are
//! a claim, never the authority: the Task owner enumerates the authoritative
//! Action attempt set from the delegation (execution lifetime), requires an
//! exact match (missing, extra, and duplicate refs are fail-closed technical
//! errors), reads the Action owner's certainty without ever changing it, and
//! additionally requires the Task-wide completion barrier (no `Unknown`
//! attempt anywhere under the same Task) inside the same short transaction as
//! the completion CAS.

use ene_primitive::{RawId, WallClockWithTz};

use crate::agent::TaskAgentOutput;
use crate::delegation::DelegationId;
use crate::task::{TaskId, TaskRef, TaskRevision};

/// Durable identity of one final Task result body.
///
/// The Task owner's orchestration mints it at the explicit finalization
/// boundary; the repository never re-allocates it. One delegation has at most
/// one final result, and retrying the same identity is idempotent.
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

/// The durable premises of one final result arrival (AU15a).
///
/// The relied Task revision is the delegation row's, never repeated here. The
/// arrival is recorded before any adoption attempt and before the final
/// result is visible; it judges nothing about currentness, certainty,
/// terminal state, or completion.
///
/// [`core::fmt::Debug`] redacts the body through [`TaskAgentOutput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentResultArrival {
    pub delegation: DelegationId,
    pub result: TaskResultId,
    pub body: TaskAgentOutput,
}

/// A caller's claim about which Action attempts a result relied on.
///
/// The claim is comparison material only: the repository enumerates the
/// authoritative set from the result's delegation and requires an exact
/// set match, so the claim can neither narrow nor widen the durable set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResultAdoptionClaim {
    pub result: TaskResultId,
    pub attempt_refs: Vec<RawId>,
}

/// One final result as durably recorded.
///
/// `task` is the relied Task revision resolved from the delegation row,
/// `attempt_refs` is exactly the result-local verified correlation stamped in
/// `task_result_attempt` (never the Task-wide barrier attempts), and
/// `adopted_revision` is `None` until the adoption commit succeeds.
///
/// [`core::fmt::Debug`] redacts the body through [`TaskAgentOutput`].
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

/// The Task owner's domain result of one adoption attempt (AU15b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResultAcceptance {
    /// The result matched the current revision and purpose identity, its
    /// authoritative set matched the claim, every relied attempt was
    /// `ConfirmedSuccess`, the Task-wide completion barrier found no
    /// `Unknown`, and the completion CAS committed in the same short
    /// transaction.
    AdoptedAsCompletion(TaskRef),
    /// The current revision moved, the Task is terminal, or a later cancel
    /// marker applies; the result stays durable against its original relied
    /// revision and the current Task is unchanged.
    RecordedToOriginalOnly,
    /// The claim matched the authoritative set, but completion is withheld:
    /// `attempts` are the blockers (relied attempts that are not
    /// `ConfirmedSuccess`, union the Task-wide `Unknown`), deduplicated.
    /// The result-local correlation is recorded; the Task is unchanged and
    /// the same result may be re-evaluated after Action-side evidence
    /// settles.
    WithheldByEffectFacts { attempts: Vec<RawId> },
    /// No result row exists for the identity; nothing was written.
    MissingResult { result: TaskResultId },
    /// The result's delegation has no durable correspondence; nothing was
    /// written.
    MissingDelegation { delegation: DelegationId },
    /// The result's Task has no durable state; nothing was written.
    MissingTask { task: TaskId },
}
