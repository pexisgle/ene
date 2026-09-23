use ene_primitive::WallClockWithTz;

use crate::identity::{SourceRangeRef, SummaryId};
use crate::scope::LearningScope;

#[derive(Clone, PartialEq, Eq)]
pub struct SummaryRecord {
    pub id: SummaryId,
    pub scope: LearningScope,
    pub content: String,
    pub source: SourceRangeRef,
    pub formed_at: WallClockWithTz,
}

impl core::fmt::Debug for SummaryRecord {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SummaryRecord")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("content", &"[redacted]")
            .field("source", &self.source)
            .field("formed_at", &self.formed_at)
            .finish()
    }
}
