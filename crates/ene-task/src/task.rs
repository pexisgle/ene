use ene_primitive::{RawId, RevisionInner, WallClockWithTz};

use crate::context::{TaskContextEntry, TaskContextEntryId, TaskContextOrigin};
use crate::result::TaskResultId;
use crate::workspace::{WorkspaceAssociation, WorkspaceAssociationPremise};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(RawId);

impl TaskId {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskRevision(RevisionInner);

impl TaskRevision {
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

    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskRef {
    pub task: TaskId,
    pub revision: TaskRevision,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskPurposeRef {
    pub task: TaskId,
    pub adopted_revision: TaskRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SteeringPremiseRef {
    pub expected: TaskRef,
    pub purpose: TaskPurposeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssigneeRef {
    pub companion: RawId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskProgress {
    Started,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl TaskProgress {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "started" => Some(Self::Started),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub reference: TaskRef,
    pub purpose: TaskPurposeRef,
    pub assignee: AssigneeRef,
    pub progress: TaskProgress,
    pub adopted_result: Option<TaskResultId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRevisionRecord {
    pub reference: TaskRef,
    pub purpose: TaskPurposeRef,
    pub purpose_text: TaskPurpose,
    pub assignee: AssigneeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub task: Task,
    pub revision: TaskRevisionRecord,
    pub context: Vec<TaskContextEntry>,
    pub workspace: Option<WorkspaceAssociation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCreationPremise {
    pub task: TaskId,
    pub purpose: TaskPurpose,
    pub entry: TaskContextEntryId,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
    pub assignee: AssigneeRef,
    pub workspace: Option<WorkspaceAssociationPremise>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCreationOutcome {
    Created(TaskRef),
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPurposeAdoptionPremise {
    pub purpose: TaskPurpose,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInstructionAdoptionPremise {
    pub entry: TaskContextEntryId,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCommitPremise {
    pub expected: TaskRef,
    pub new_purpose: Option<TaskPurposeAdoptionPremise>,
    pub adopted_purpose_entry: TaskContextEntryId,
    pub adopted_instruction: Option<TaskInstructionAdoptionPremise>,
}
