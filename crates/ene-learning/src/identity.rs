use ene_primitive::{RawId, RevisionInner};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SummaryId(RawId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LearningClaimRef(RawId);

impl LearningClaimRef {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryRevision(RevisionInner);

impl MemoryRevision {
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
pub enum ExperienceSourceKind {
    Dialogue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRangeRef {
    pub kind: ExperienceSourceKind,
    pub start: RawId,
    pub end: RawId,
}
