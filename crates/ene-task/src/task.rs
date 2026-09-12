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

    /// Returns the successor revision, or [`None`] at [`u64::MAX`].
    ///
    /// Exhaustion is reported rather than hidden: a silent maximum step would
    /// make a new revision indistinguishable from its predecessor and break
    /// compare-before-commit.
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
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

/// A steering caller's relied-on Task revision and purpose.
///
/// This is comparison material, not authority: the Task owner compares it
/// against the durable current state and returns a stale outcome on
/// mismatch. `purpose` must be the purpose identity in force at
/// `expected.revision`; [`orchestrate_steering`](crate::orchestrate_steering)
/// checks that correspondence before committing, and a purpose text match is
/// never used as identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SteeringPremiseRef {
    /// The current revision the caller relied on.
    pub expected: TaskRef,
    /// The purpose identity the caller relied on at `expected.revision`.
    pub purpose: TaskPurposeRef,
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

/// The committed current unit of one Task: the AU2 creation plus every AU4
/// steering forward.
///
/// The context spans the current revision's adopted-purpose entry and the
/// adopted-instruction entries in force (adopted at or before the current
/// revision); each entry keeps its own adoption reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub task: Task,
    /// The snapshot of the current revision.
    pub revision: TaskRevisionRecord,
    /// The current revision's adopted-purpose entry, followed by every
    /// adopted-instruction entry in force.
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

/// A purpose adoption proposed by one steering commit (AU4).
///
/// The repository stamps the adopted revision (`expected.revision + 1`) after
/// the CAS succeeds; the caller supplies the text and the provenance, and
/// never names a future revision. For steering, `origin` is the same
/// utterance record as the adopted instruction's: kind
/// [`TaskContextOriginKind::OwnerConversation`](crate::TaskContextOriginKind::OwnerConversation)
/// with `source` equal to the steering proposal's instruction source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPurposeAdoptionPremise {
    pub purpose: TaskPurpose,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
}

/// An instruction adoption proposed by one steering commit (AU4).
///
/// `entry` is the adoption identity itself: the Task owner's orchestration
/// mints it and passes it in [`TaskCommitPremise::adopted_instruction`]; the
/// repository never re-mints it and stamps only the post-CAS `(task,
/// revision)` reference. The entry is written once at the adoption revision
/// and never re-recorded by a later forward. `origin.source` references the
/// utterance record and never copies its body; steering always uses kind
/// [`TaskContextOriginKind::OwnerConversation`](crate::TaskContextOriginKind::OwnerConversation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInstructionAdoptionPremise {
    /// Identity of the context entry recording the adopted instruction.
    pub entry: TaskContextEntryId,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
}

/// The premise for one steering commit (AU4).
///
/// Advances the Task by exactly one revision. The repository writes the new
/// revision snapshot, the new revision's adopted-purpose context entry, the
/// adopted-instruction context entry when one is proposed, and the current
/// pointer in one atomic commit; every older revision and context entry is
/// retained. The caller never names the successor revision: it supplies the
/// relied-on `expected` revision and the repository stamps the post-CAS
/// `(task, revision)` references. Other context kinds arrive with the
/// producers that can identify them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCommitPremise {
    pub expected: TaskRef,
    /// `Some` adopts a new purpose at the new revision; `None` carries the
    /// current purpose and its adopted-purpose context entry forward.
    pub new_purpose: Option<TaskPurposeAdoptionPremise>,
    /// Identity of the context entry the new revision records for the adopted
    /// purpose, in both the change and carry-forward branches. The Task owner
    /// mints it; the repository never allocates it and stamps only the
    /// post-CAS `(task, revision)` reference and adopted revision.
    pub adopted_purpose_entry: TaskContextEntryId,
    /// The instruction adopted by this forward. Steering always passes
    /// `Some`: the instruction source is mandatory in the steering proposal,
    /// and its origin is the same utterance record as a purpose change (kind
    /// [`TaskContextOriginKind::OwnerConversation`](crate::TaskContextOriginKind::OwnerConversation),
    /// source equal to the instruction source). `None` is for a future
    /// forward that adopts no instruction; the entry is written once and
    /// never re-recorded.
    pub adopted_instruction: Option<TaskInstructionAdoptionPremise>,
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
    fn revision_successor_reports_exhaustion_instead_of_aliasing_the_maximum() {
        assert_eq!(
            TaskRevision::initial().checked_next(),
            Some(TaskRevision::from_u64(2))
        );
        assert_eq!(TaskRevision::from_u64(u64::MAX).checked_next(), None);
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
