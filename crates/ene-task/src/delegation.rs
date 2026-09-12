//! Delegation creation foundation (AU3).
//!
//! A [`DelegationRef`] is the durable correspondence between one Task
//! revision and one temporary Task Agent: it records who delegated, which
//! revision the delegation relies on, the ephemeral agent identity, and the
//! workspace boundary copied at creation. The correspondence is durable; the
//! agent is not. A persisted row never proves the agent is alive, the
//! delegated work is running, or the Task is complete.
//!
//! This slice creates and reloads the correspondence only. It contains no
//! LLM, Action, or permission behavior: executing delegated work, checking
//! current permissions, resolving paths, and deciding the Task's completion
//! belong to the owners of those behaviors and are re-checked against
//! current state there.

use ene_primitive::RawId;

use crate::task::{AssigneeRef, TaskId, TaskRef};
use crate::workspace::{WorkspaceAssocId, WorkspaceFolderRef};

/// Identity of one delegation correspondence. Wraps [`RawId`]; never reused.
///
/// Re-delegation and retries start a new delegation identity; an existing
/// identity is never replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DelegationId(RawId);

impl DelegationId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// Identity of one temporary Task Agent execution subject.
///
/// The agent is ephemeral: it owns no durable data, and a persisted
/// delegation row never proves it is alive. After a restart only the
/// correspondence is restored, the execution context is treated as lost, and
/// the agent is never automatically restarted or the Task automatically
/// resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskAgentEphemeralId(RawId);

impl TaskAgentEphemeralId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }

    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }
}

/// The workspace boundary as it stood when one delegation was created.
///
/// This is a projection of the workspace association, frozen so the
/// delegation can explain which boundary it relied on even after the
/// association changes. It is not a permission and not a path-resolution
/// source: the workspace association stays the single master, and current
/// validity is re-checked from it, never from this copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedWorkspace {
    /// The association identity the copy came from (provenance).
    pub assoc: WorkspaceAssocId,
    /// The folder at creation time.
    pub folder: WorkspaceFolderRef,
    /// The save target at creation time; `None` when it was undecided.
    pub save_target: Option<WorkspaceFolderRef>,
}

/// The boundary copy carried by one delegation.
///
/// `workspace` is `None` when the delegation uses no workspace boundary. The
/// copy is provenance and comparison material, never authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationScope {
    pub workspace: Option<DelegatedWorkspace>,
}

/// The durable correspondence of one delegation.
///
/// `task` is the relied-on revision: the delegation's purpose is resolved
/// from that revision's `task_revision` snapshot and is never copied here.
/// `delegator` is the Task-side assignee as read inside the AU3 commit; it
/// is a copy, and current validity is re-checked from the Task's current
/// unit. `agent` is ephemeral, not a durable owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationRef {
    pub delegation: DelegationId,
    pub task: TaskRef,
    pub delegator: AssigneeRef,
    pub agent: TaskAgentEphemeralId,
    pub scope: DelegationScope,
}

/// A request to create one delegation for a Task revision (H-A).
///
/// The delegation and agent identities are minted by
/// [`orchestrate_delegation`](crate::orchestrate_delegation), never by the
/// caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDelegationCommand {
    /// The relied-on Task revision (boundary token), compared against the
    /// durable current revision before and inside the creation commit.
    pub task: TaskRef,
    /// The workspace boundary copied at creation.
    pub scope_copy: DelegationScope,
}

/// The full content of one AU3 delegation creation.
///
/// [`orchestrate_delegation`](crate::orchestrate_delegation) mints
/// `delegation` and `agent` and passes them here; the repository never
/// re-allocates them. `delegator` is deliberately absent: the repository
/// copies it from the current Task row inside the atomic commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationCreationPremise {
    pub delegation: DelegationId,
    /// The expected Task revision (boundary token).
    pub task: TaskRef,
    pub agent: TaskAgentEphemeralId,
    pub scope_copy: DelegationScope,
}

/// The domain result of one delegation creation (AU3).
///
/// Stale and missing are `Ok`-side domain outcomes, not technical errors,
/// and leave no writes behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationOutcome {
    /// The correspondence was committed.
    Delegated(DelegationRef),
    /// The expected Task revision no longer matches; nothing was changed.
    StaleTaskRevision { current: TaskRef },
    /// The premise names a Task with no durable state; nothing was changed.
    MissingTask { task: TaskId },
}

#[cfg(test)]
mod tests {
    use super::{DelegationId, TaskAgentEphemeralId};

    #[test]
    fn generated_ids_are_distinct() {
        assert_ne!(DelegationId::generate(), DelegationId::generate());
        assert_ne!(
            TaskAgentEphemeralId::generate(),
            TaskAgentEphemeralId::generate()
        );
    }
}
