//! Adopted context entries for one Task revision.

use ene_primitive::{RawId, WallClockWithTz};

use crate::task::{TaskPurposeRef, TaskRef};

/// Identity of one adopted context entry. Wraps [`RawId`]; never reused.
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

/// Which adopted identity one context entry records.
///
/// AU2 records the adopted purpose. Instruction, material, and working
/// understanding entries arrive with the slices that adopt them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskContextItem {
    AdoptedPurpose(TaskPurposeRef),
}

/// Where an adopted item came from. The source identity is never a copy of
/// the originating record's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskContextOrigin {
    pub kind: TaskContextOriginKind,
    pub source: RawId,
}

/// The origin kind of an adopted context item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskContextOriginKind {
    /// An Owner conversation, sourced from a History record.
    OwnerConversation,
    /// The Companion's own initiative, sourced from an activity record.
    Spontaneous,
    /// A Schedule occurrence.
    ScheduleOccurrence,
}

/// One adopted context entry recorded for a Task revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContextEntry {
    pub entry: TaskContextEntryId,
    /// The revision the item was adopted under.
    pub reference: TaskRef,
    pub item: TaskContextItem,
    pub origin: TaskContextOrigin,
    pub acquired_at: WallClockWithTz,
}

#[cfg(test)]
mod tests {
    use super::TaskContextEntryId;

    #[test]
    fn context_entry_id_round_trips_through_raw() {
        let entry = TaskContextEntryId::generate();
        assert_eq!(TaskContextEntryId::from_raw(entry.as_raw()), entry);
    }
}
