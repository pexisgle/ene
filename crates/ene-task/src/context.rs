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
/// AU2 and AU4 record the adopted purpose. The H-A steering wiring records
/// the adopted instruction; material and working understanding entries arrive
/// with the slices that adopt them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskContextItem {
    AdoptedPurpose(TaskPurposeRef),
    /// An additional instruction adopted by the Owner conversation.
    ///
    /// The adoption identity is the enclosing [`TaskContextEntryId`]; the
    /// `origin.source` of the entry references the utterance record, whose
    /// body stays canonical there and is never copied into the entry. The
    /// entry is written once at the adoption revision and never re-recorded
    /// by a later forward. Currently effective instructions are every
    /// `AdoptedInstruction` entry up to the current revision; retire exists
    /// only in a later producer slice and works by superseding the entry
    /// identity rather than deleting or rewriting it.
    AdoptedInstruction,
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
    /// The revision the entry belongs to. The adopted identity in `item` may
    /// point at an earlier revision (a carried-forward purpose), so the two
    /// are never compared by revision equality.
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
