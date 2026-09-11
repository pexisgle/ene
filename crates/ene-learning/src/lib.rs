//! Learning ownership: Experience Summary evidence, Memory current
//! recognition, and their revisions and grounds.
//!
//! A [`Memory`] is the current recognition a Companion uses later; a
//! [`SummaryRecord`] is the compressed evidence one formation or update judged
//! from, never a second current-knowledge store. A [`MemoryRevisionRecord`]
//! keeps the change history so a correction can be distinguished from a
//! situation that changed, and normal forgetting never deletes content.
//!
//! This crate owns the semantics and the [`LearningRepository`] contract; the
//! persistent implementation lives behind that trait (see `ene-store`), and
//! inference is supplied as an opaque port by the caller, so this crate takes
//! no dependency on history, task, permission, credential, or inference
//! crates. Cross-domain identities arrive as `RawId` premises and are never
//! converted into another domain's newtype.
//!
//! Stage 3 implements the Companion scope only. Global scope is deliberately
//! absent: a companion-derived memory cannot be widened by accident, and the
//! scope field keeps the distinction explicit for the later stage.

mod formation;
mod identity;
mod memory;
mod repository;
mod scope;
mod summary;

#[cfg(test)]
mod test_support;

pub use formation::{
    ChangeRejection, ExperienceCandidate, ExperienceRole, ExperienceTurn, FormationChange,
    FormationDecision, LearningInference, LearningInferenceError, MAX_FORMATION_TURNS,
    MAX_FORMED_MEMORIES, SecretScrubError, SecretScrubber, form_experience,
};
pub use identity::{ExperienceSourceKind, MemoryId, MemoryRevision, SourceRangeRef, SummaryId};
pub use memory::{ChangeKind, Importance, Memory, MemoryRevisionRecord, TemporalMeaning};
pub use repository::{
    LearningRepository, LearningTechnicalError, MemoryChange, MemoryChangeCommit,
    MemoryChangeOutcome, MemoryTarget,
};
pub use scope::LearningScope;
pub use summary::SummaryRecord;

#[cfg(test)]
mod tests {
    use ene_primitive::{RawId, WallClockWithTz};

    use super::{
        ChangeKind, ExperienceSourceKind, Importance, LearningScope, Memory, MemoryId,
        MemoryRevision, MemoryRevisionRecord, SourceRangeRef, SummaryId, SummaryRecord,
        TemporalMeaning,
    };

    fn clock() -> WallClockWithTz {
        WallClockWithTz::now()
    }

    #[test]
    fn importance_stays_bounded_and_defaults_mid_range() {
        assert_eq!(Importance::clamped(0).as_u8(), Importance::MIN);
        assert_eq!(Importance::clamped(9).as_u8(), Importance::MAX);
        assert_eq!(Importance::clamped(4).as_u8(), 4);
        assert_eq!(Importance::default().as_u8(), 3);
    }

    #[test]
    fn forgetting_is_suppression_not_a_revision_gap() {
        assert!(ChangeKind::Forgotten.suppresses_recall());
        for other in [
            ChangeKind::Initial,
            ChangeKind::Reinforced,
            ChangeKind::Refined,
            ChangeKind::Integrated,
            ChangeKind::CorrectedInitiallyWrong,
            ChangeKind::ChangedSince,
        ] {
            assert!(!other.suppresses_recall(), "{other:?} must not suppress");
        }
        assert_ne!(
            ChangeKind::CorrectedInitiallyWrong,
            ChangeKind::ChangedSince,
            "initially-wrong and changed-since stay distinct"
        );
    }

    #[test]
    fn revision_exhaustion_reports_none_instead_of_aliasing() {
        assert_eq!(MemoryRevision::initial().as_u64(), 1);
        assert_eq!(
            MemoryRevision::initial().checked_next(),
            Some(MemoryRevision::from_u64(2))
        );
        assert_eq!(MemoryRevision::from_u64(u64::MAX).checked_next(), None);
    }

    #[test]
    fn scope_carries_the_companion_premise() {
        let companion = RawId::new();
        let scope = LearningScope::companion(companion);
        assert_eq!(scope.companion_id(), companion);
    }

    #[test]
    fn source_range_keeps_history_references() {
        let start = RawId::new();
        let end = RawId::new();
        let source = SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start,
            end,
        };
        assert_eq!(source.start, start);
        assert_eq!(source.end, end);
        assert_eq!(source.kind, ExperienceSourceKind::Dialogue);
    }

    #[test]
    fn debug_redacts_content_and_keeps_references() {
        let content = String::from("probe-memory-content");
        let memory = Memory {
            id: MemoryId::generate(),
            revision: MemoryRevision::initial(),
            scope: LearningScope::companion(RawId::new()),
            content: content.clone(),
            importance: Importance::clamped(4),
            temporal: TemporalMeaning::Enduring,
            recall_suppressed: false,
            updated_at: clock(),
        };
        let rendered = format!("{memory:?}");
        assert!(!rendered.contains(&content), "content redacted: {rendered}");
        assert!(rendered.contains("Enduring"), "meaning stays: {rendered}");

        let revision = MemoryRevisionRecord {
            memory: memory.id,
            revision: MemoryRevision::initial(),
            scope: memory.scope,
            content: content.clone(),
            importance: memory.importance,
            temporal: TemporalMeaning::Enduring,
            change: ChangeKind::Initial,
            recall_suppressed: false,
            summary: None,
            at: clock(),
        };
        let rendered = format!("{revision:?}");
        assert!(!rendered.contains(&content), "content redacted: {rendered}");

        let summary = SummaryRecord {
            id: SummaryId::generate(),
            scope: memory.scope,
            content: content.clone(),
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            formed_at: clock(),
        };
        let rendered = format!("{summary:?}");
        assert!(!rendered.contains(&content), "content redacted: {rendered}");
    }
}
