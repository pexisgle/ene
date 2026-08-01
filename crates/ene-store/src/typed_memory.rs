//! Typed memory domain model — re-exported from `ene-core`.
//!
//! These types are pure domain vocabulary (kinds, statuses, scores, queries)
//! with no `SeaORM`/SQL concerns, so their definitions live in `ene-core`.
//! This module stays so existing `ene_store::typed_memory::*` /
//! `ene_store::MemoryKind`-style import paths keep working unchanged.

pub use ene_core::{
    AffectAnnotation, HybridSearchWeights, MemoryCandidateSource, MemoryConfidence, MemoryItem,
    MemoryJournalListOptions, MemoryKind, MemorySalience, MemoryScope, MemoryScoreBreakdown,
    MemorySearchOptions, MemorySource, MemoryStatus, NewMemoryItem, Query, ScoredMemory, TimeRange,
};
