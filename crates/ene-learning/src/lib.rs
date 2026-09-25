mod formation;
mod identity;
mod memory;
mod recall;
mod relevance;
mod repository;
mod scope;
mod summary;

#[doc(no_inline)]
pub use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
pub use formation::{
    ExperienceCandidate, ExperienceRole, ExperienceTurn, FormationDecision, LearningInference,
    LearningInferenceAnswer, LearningInferenceError, LearningInferencePremise, form_experience,
};
pub use identity::{
    ExperienceSourceKind, LearningClaimRef, MemoryId, MemoryRevision, SourceRangeRef, SummaryId,
};
pub use memory::{ChangeKind, Importance, Memory, MemoryRevisionRecord, TemporalMeaning};
pub use recall::{RecallQuery, RecalledMemory, recall};
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
        ChangeKind, Importance, LearningScope, Memory, MemoryChange, MemoryId, MemoryRevision,
        MemoryRevisionRecord, MemoryTarget, SourceRangeRef, SummaryId, SummaryRecord,
        TemporalMeaning,
    };

    #[test]
    fn importance_and_revision_boundaries_report_without_aliasing() {
        for (value, expected) in [(0, Importance::MIN), (4, 4), (9, Importance::MAX)] {
            assert_eq!(Importance::clamped(value).as_u8(), expected);
        }
        assert_eq!(Importance::default().as_u8(), 3);
        assert_eq!(MemoryRevision::initial().as_u64(), 1);
        assert_eq!(
            MemoryRevision::initial().checked_next(),
            Some(MemoryRevision::from_u64(2))
        );
        assert_eq!(MemoryRevision::from_u64(u64::MAX).checked_next(), None);
    }

    #[test]
    fn forgetting_is_recall_suppression_not_targeted_deletion() {
        assert!(ChangeKind::Forgotten.suppresses_recall());
        for change in [
            ChangeKind::Initial,
            ChangeKind::Reinforced,
            ChangeKind::Refined,
            ChangeKind::Integrated,
            ChangeKind::CorrectedInitiallyWrong,
            ChangeKind::ChangedSince,
        ] {
            assert!(
                !change.suppresses_recall(),
                "ordinary learning changes remain distinct from forgetting: {change:?}"
            );
        }
        assert_ne!(
            ChangeKind::CorrectedInitiallyWrong,
            ChangeKind::ChangedSince,
            "an initially-wrong correction remains distinct from a later change"
        );
    }

    #[test]
    fn public_learning_debug_values_redact_bodies() {
        let body = String::from("private-memory-body");
        let at = WallClockWithTz::now();
        let scope = LearningScope::companion(RawId::new());
        let memory = Memory {
            id: MemoryId::generate(),
            revision: MemoryRevision::initial(),
            scope,
            content: body.clone(),
            importance: Importance::clamped(4),
            temporal: TemporalMeaning::Enduring,
            recall_suppressed: false,
            updated_at: at,
        };
        let revision = MemoryRevisionRecord {
            memory: memory.id,
            revision: memory.revision,
            scope,
            content: body.clone(),
            importance: memory.importance,
            temporal: memory.temporal,
            change: ChangeKind::Initial,
            recall_suppressed: false,
            summary: None,
            at,
        };
        let summary = SummaryRecord {
            id: SummaryId::generate(),
            scope,
            content: body.clone(),
            source: SourceRangeRef {
                kind: super::ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            formed_at: at,
        };
        let change = MemoryChange {
            target: MemoryTarget::New {
                id: MemoryId::generate(),
            },
            scope,
            content: body.clone(),
            importance: Importance::default(),
            temporal: TemporalMeaning::Enduring,
            change: ChangeKind::Initial,
            at,
        };

        for rendered in [
            format!("{memory:?}"),
            format!("{revision:?}"),
            format!("{summary:?}"),
            format!("{change:?}"),
        ] {
            assert!(!rendered.contains(&body), "body redacted: {rendered}");
        }
    }
}
