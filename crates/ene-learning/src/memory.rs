use ene_primitive::WallClockWithTz;

use crate::identity::{MemoryId, MemoryRevision, SummaryId};
use crate::scope::LearningScope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Importance(u8);

impl Importance {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 5;

    #[must_use]
    pub fn clamped(value: u8) -> Self {
        Self(value.clamp(Self::MIN, Self::MAX))
    }

    #[must_use]
    pub fn as_u8(self) -> u8 {
        self.0
    }
}

impl Default for Importance {
    fn default() -> Self {
        Self(3)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemporalMeaning {
    Enduring,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Initial,
    Reinforced,
    Refined,
    Integrated,
    CorrectedInitiallyWrong,
    ChangedSince,
    Forgotten,
}

impl ChangeKind {
    #[must_use]
    pub fn suppresses_recall(self) -> bool {
        matches!(self, Self::Forgotten)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Memory {
    pub id: MemoryId,
    pub revision: MemoryRevision,
    pub scope: LearningScope,
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    pub recall_suppressed: bool,
    pub updated_at: WallClockWithTz,
}

impl core::fmt::Debug for Memory {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Memory")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("scope", &self.scope)
            .field("content", &"[redacted]")
            .field("importance", &self.importance)
            .field("temporal", &self.temporal)
            .field("recall_suppressed", &self.recall_suppressed)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MemoryRevisionRecord {
    pub memory: MemoryId,
    pub revision: MemoryRevision,
    pub scope: LearningScope,
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    pub change: ChangeKind,
    pub recall_suppressed: bool,
    pub summary: Option<SummaryId>,
    pub at: WallClockWithTz,
}

impl core::fmt::Debug for MemoryRevisionRecord {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MemoryRevisionRecord")
            .field("memory", &self.memory)
            .field("revision", &self.revision)
            .field("scope", &self.scope)
            .field("content", &"[redacted]")
            .field("importance", &self.importance)
            .field("temporal", &self.temporal)
            .field("change", &self.change)
            .field("recall_suppressed", &self.recall_suppressed)
            .field("summary", &self.summary)
            .field("at", &self.at)
            .finish()
    }
}
