//! Durable identities and the source range one Summary grounds on.

use ene_primitive::{RawId, RevisionInner};

/// Identity of one Memory. Wraps [`RawId`]; never converted to any other
/// domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryId(RawId);

impl MemoryId {
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

/// Identity of one Experience Summary. Wraps [`RawId`]; never converted to
/// any other domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SummaryId(RawId);

impl SummaryId {
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

/// Monotonic order of one Memory's revisions.
///
/// Follows the [`RevisionInner`] discipline: the inner count travels only
/// inside its `(MemoryId, MemoryRevision)` pair, and [`Self::checked_next`]
/// reports exhaustion instead of aliasing `u64::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryRevision(RevisionInner);

impl MemoryRevision {
    /// The revision of a newly formed Memory.
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

    /// Callers must treat [`None`] as revision exhaustion and refuse the
    /// commit rather than writing `u64::MAX` again with new content.
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

/// Which activity produced the Experience behind a Summary.
///
/// Stage 3 forms from dialogue only; the other Experience kinds arrive with
/// their owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExperienceSourceKind {
    Dialogue,
}

/// The coarse source range a Summary was compressed from.
///
/// `start` and `end` are History message identities in the conversation that
/// produced the Experience, so the current recognition can always be traced
/// back towards the retained record. This is a reference, not a copy: the raw
/// text is owned by Conversation History and is never duplicated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRangeRef {
    pub kind: ExperienceSourceKind,
    pub start: RawId,
    pub end: RawId,
}
