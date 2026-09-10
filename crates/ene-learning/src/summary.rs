//! Compressed Experience evidence and its source range.

use ene_primitive::WallClockWithTz;

use crate::identity::{SourceRangeRef, SummaryId};
use crate::scope::LearningScope;

/// One compressed piece of evidence a formation or update was judged from.
///
/// A Summary is not current knowledge and not a copy of Conversation History:
/// it holds what happened and the context needed to explain a Memory change,
/// while the exact wording stays in the retained History referenced by
/// [`Self::source`].
#[derive(Clone, PartialEq, Eq)]
pub struct SummaryRecord {
    pub id: SummaryId,
    pub scope: LearningScope,
    /// Compressed evidence text; redacted from [`core::fmt::Debug`].
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
