mod formation;
mod identity;
mod memory;
mod recall;
mod relevance;
mod repository;
mod scope;
mod summary;

#[cfg(test)]
mod test_support;

#[doc(no_inline)]
pub use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
pub use formation::{
    ExperienceCandidate, ExperienceRole, ExperienceTurn, FormationDecision, LearningInference,
    LearningInferenceAnswer, LearningInferenceError, LearningInferencePremise,
    MAX_FORMATION_CHANGES, MAX_FORMATION_TURNS, form_experience,
};
pub use identity::{
    ExperienceSourceKind, LearningClaimRef, MemoryId, MemoryRevision, SourceRangeRef, SummaryId,
};
pub use memory::{ChangeKind, Importance, Memory, MemoryRevisionRecord, TemporalMeaning};
pub use recall::{RECALL_CANDIDATE_LIMIT, RecallQuery, RecalledMemory, recall};
pub use relevance::recall_index_terms;
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
        ChangeKind, ExperienceSourceKind, Importance, LearningScope, Memory, MemoryChange,
        MemoryId, MemoryRevision, MemoryRevisionRecord, MemoryTarget, SourceRangeRef, SummaryId,
        SummaryRecord, TemporalMeaning,
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

        let change = MemoryChange {
            target: MemoryTarget::New {
                id: MemoryId::generate(),
            },
            scope: memory.scope,
            content: content.clone(),
            importance: Importance::default(),
            temporal: TemporalMeaning::Enduring,
            change: ChangeKind::Initial,
            recall_suppressed: false,
            at: clock(),
        };
        let rendered = format!("{change:?}");
        assert!(!rendered.contains(&content), "content redacted: {rendered}");
    }
}
