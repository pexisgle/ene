//! Task identity, revision, and the reference pair.

use ene_primitive::{RawId, RevisionInner};

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

#[cfg(test)]
mod tests {
    use super::{TaskId, TaskRef, TaskRevision};

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
}
