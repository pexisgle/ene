use ene_primitive::{RawId, WallClockWithTz};

use crate::task::{TaskPurposeRef, TaskRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskContextEntryId(RawId);

impl TaskContextEntryId {
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
pub enum TaskContextItem {
    AdoptedPurpose(TaskPurposeRef),
    AdoptedInstruction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskContextOrigin {
    pub kind: TaskContextOriginKind,
    pub source: RawId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskContextOriginKind {
    OwnerConversation,
    OwnerManagement,
    Spontaneous,
    ScheduleOccurrence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContextEntry {
    pub entry: TaskContextEntryId,
    pub reference: TaskRef,
    pub item: TaskContextItem,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
}
