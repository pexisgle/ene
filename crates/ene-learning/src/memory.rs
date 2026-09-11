//! Memory current recognition and its change history.

use ene_primitive::WallClockWithTz;

use crate::identity::{MemoryId, MemoryRevision, SummaryId};
use crate::scope::LearningScope;

/// How much the current recognition matters for later recall.
///
/// Importance is a semantic judgement, independent of scope and distinct from
/// a query-time retrieval score. It is bounded so a model cannot unbalance
/// recall with an unbounded number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Importance(u8);

impl Importance {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 5;

    /// Clamps a caller- or model-supplied value into the bounded range.
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

/// How the current recognition relates to time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemporalMeaning {
    /// A fact, preference, or interpretation that holds until it changes.
    Enduring,
    /// Something that happened at a point in time and stays true as history.
    Event,
}

/// What changed between a Memory revision and its predecessor.
///
/// [`Self::CorrectedInitiallyWrong`] and [`Self::ChangedSince`] keep the two
/// correction meanings distinct: the first says the earlier recognition was
/// never valid, the second says it was valid until the situation changed. Both
/// preserve the earlier revision and its evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// The first formation of this Memory.
    Initial,
    /// The same information arrived again; the recognition was confirmed.
    Reinforced,
    /// The recognition became more precise without being contradicted.
    Refined,
    /// Information from another Experience was integrated into this one.
    Integrated,
    /// The earlier recognition was wrong from the start.
    CorrectedInitiallyWrong,
    /// The earlier recognition was valid until the situation changed.
    ChangedSince,
    /// A normal forgetting request suppressed recall without deleting content.
    Forgotten,
}

impl ChangeKind {
    /// Whether this change suppresses recall.
    #[must_use]
    pub fn suppresses_recall(self) -> bool {
        matches!(self, Self::Forgotten)
    }
}

/// The current recognition of one Memory.
#[derive(Clone, PartialEq, Eq)]
pub struct Memory {
    pub id: MemoryId,
    /// Always present; the current value of the revision chain.
    pub revision: MemoryRevision,
    pub scope: LearningScope,
    /// Current recognition text; redacted from [`core::fmt::Debug`].
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    /// Normal forgetting is recall suppression, never deletion: a suppressed
    /// Memory keeps its content, revisions, and grounds.
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

/// One past revision of a Memory, kept as the change history.
///
/// The revision record is the authority for what changed and why; the current
/// row is not a replacement for it. `summary` names the evidence this revision
/// was judged from, when the change was grounded on a Summary.
#[derive(Clone, PartialEq, Eq)]
pub struct MemoryRevisionRecord {
    pub memory: MemoryId,
    pub revision: MemoryRevision,
    pub scope: LearningScope,
    /// Content at this revision; redacted from [`core::fmt::Debug`].
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    pub change: ChangeKind,
    pub recall_suppressed: bool,
    /// Evidence Summary for this revision, when one was recorded.
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
