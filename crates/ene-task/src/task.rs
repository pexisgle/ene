//! Task identity, purpose, current state, and revision records.

use ene_primitive::{RawId, RevisionInner, WallClockWithTz};

use crate::context::{TaskContextEntry, TaskContextEntryId, TaskContextOrigin};
use crate::workspace::{WorkspaceAssociation, WorkspaceAssociationPremise};

/// Identity of one Task. Wraps [`RawId`]; never converted to any other domain
/// newtype and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(RawId);

impl TaskId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// Monotonic order of one Task's revisions, as decided by the Task owner.
///
/// Follows the [`RevisionInner`] discipline: the number travels only inside
/// its `(TaskId, TaskRevision)` pair, and a larger value never proves newer
/// across different tasks. A Task purpose change is a forward step of this
/// revision, never a change of some other generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskRevision(RevisionInner);

impl TaskRevision {
    /// The revision of a newly created Task.
    #[must_use]
    pub fn initial() -> Self {
        Self(RevisionInner::from_u64(1))
    }

    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(RevisionInner::from_u64(value))
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0.as_u64()
    }
}

/// One Task identity together with the revision of its state.
///
/// This is the boundary token a caller passes so the owner can compare the
/// revision it expects against the durable current one. It is comparison
/// material, not authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskRef {
    pub task: TaskId,
    pub revision: TaskRevision,
}

/// The adopted purpose text.
///
/// This is content, not identity: [`TaskPurposeRef`] identifies where the
/// purpose was adopted, and the text is stored once in that revision's
/// snapshot. Redacted from [`core::fmt::Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct TaskPurpose {
    pub text: String,
}

impl core::fmt::Debug for TaskPurpose {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskPurpose")
            .field("text", &"[redacted]")
            .finish()
    }
}

/// Identity of a purpose adopted by one Task revision.
///
/// The purpose text is canonical in the `task_revision` snapshot at
/// `adopted_revision`; a revision that does not change the purpose carries
/// the predecessor's reference forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskPurposeRef {
    pub task: TaskId,
    pub adopted_revision: TaskRevision,
}

/// The Companion a Task is assigned to, as a Task-owned premise.
///
/// Carries [`RawId`] and is never converted from or into another domain's
/// Companion newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssigneeRef {
    pub companion: RawId,
}

/// The current durable state of one Task (D1).
///
/// The purpose text is not duplicated here; it is read from the current
/// revision snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub reference: TaskRef,
    pub purpose: TaskPurposeRef,
    pub assignee: AssigneeRef,
}

/// One revision of a Task, kept as the change history (D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRevisionRecord {
    pub reference: TaskRef,
    /// The adopted purpose in force at this revision.
    pub purpose: TaskPurposeRef,
    /// The purpose text snapshot for this revision.
    pub purpose_text: TaskPurpose,
    pub assignee: AssigneeRef,
}

/// The committed AU2 unit of one Task at its current revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub task: Task,
    /// The snapshot of the current revision.
    pub revision: TaskRevisionRecord,
    /// The context entries recorded for the current revision.
    pub context: Vec<TaskContextEntry>,
    pub workspace: Option<WorkspaceAssociation>,
}

/// The premise for creating one Task (AU2).
///
/// Identities are minted by the Task owner before the repository call; the
/// repository writes the whole premise in one atomic commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCreationPremise {
    pub task: TaskId,
    pub purpose: TaskPurpose,
    /// Identity of the initial context entry adopting the purpose.
    pub entry: TaskContextEntryId,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
    pub assignee: AssigneeRef,
    /// The confirmed workspace association, when the Task has one.
    pub workspace: Option<WorkspaceAssociationPremise>,
}

#[cfg(test)]
mod tests {
    use ene_primitive::RawId;

    use super::{AssigneeRef, TaskId, TaskPurpose, TaskPurposeRef, TaskRef, TaskRevision};

    #[test]
    fn initial_task_revision_is_one() {
        assert_eq!(TaskRevision::initial().as_u64(), 1);
        assert_eq!(TaskRevision::from_u64(7).as_u64(), 7);
        assert_ne!(TaskRevision::initial(), TaskRevision::from_u64(2));
    }

    #[test]
    fn task_refs_keep_identity_and_revision_as_one_pair() {
        let first = TaskRef {
            task: TaskId::generate(),
            revision: TaskRevision::initial(),
        };
        let second = TaskRef {
            task: TaskId::generate(),
            revision: TaskRevision::initial(),
        };
        assert_ne!(first, second, "different identities are different refs");
        assert_ne!(
            TaskRef {
                task: first.task,
                revision: TaskRevision::from_u64(2),
            },
            first,
            "the revision stays part of the pair"
        );
    }

    #[test]
    fn purpose_ref_identifies_the_adoption_revision() {
        let task = TaskId::generate();
        let first = TaskPurposeRef {
            task,
            adopted_revision: TaskRevision::initial(),
        };
        let second = TaskPurposeRef {
            task,
            adopted_revision: TaskRevision::from_u64(2),
        };
        assert_ne!(
            first, second,
            "the adoption revision stays part of the purpose identity"
        );
    }

    #[test]
    fn debug_redacts_the_purpose_text() {
        let purpose = TaskPurpose {
            text: String::from("probe-task-purpose"),
        };
        let rendered = format!("{purpose:?}");
        assert!(!rendered.contains("probe-task-purpose"));
    }

    #[test]
    fn assignee_ref_carries_the_raw_companion_identity() {
        let companion = RawId::new();
        let assignee = AssigneeRef { companion };
        assert_eq!(assignee.companion, companion);
    }
}
