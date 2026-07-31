use ene_core::{
    MemoryCandidateSource, MemoryItem, MemoryKind, MemoryScoreBreakdown, MemorySource, ScoredMemory,
};
use serde::{Deserialize, Serialize};

/// Minimum `emotional_match` score required to classify a recall as emotional continuity.
///
/// Values below this threshold are treated as incidental affect noise from the current turn
/// rather than the primary recall driver.
pub const EMOTIONAL_MATCH_REASON_THRESHOLD: f32 = 0.85;

/// Human-readable reason why a memory was recalled.
///
/// Each recalled memory carries exactly one primary reason for UX, debug output,
/// and prompt introspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallReason {
    /// Semantic similarity or hybrid score matched the current topic.
    SimilarTopic,
    /// Recent conversation or episodic context surfaced this memory.
    RecentConversation,
    /// Active companion commitment or promise ledger entry.
    ActivePromise,
    /// Character lorebook / CCv3-derived semantic memory.
    CharacterLore,
    /// User preference or profile trait.
    UserPreference,
    /// Emotional continuity or affect match with the current turn.
    EmotionalContinuity,
    /// User-pinned memory (reserved for future pin UX).
    Pinned,
}

/// A typed memory recalled with an explainable reason and score breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecalledMemory {
    /// The recalled memory item.
    pub item: MemoryItem,
    /// Primary reason this memory was surfaced.
    pub reason: RecallReason,
    /// Explainable hybrid score components.
    pub score_breakdown: MemoryScoreBreakdown,
    /// Recall sources that contributed this candidate.
    pub sources: Vec<MemoryCandidateSource>,
}

impl RecalledMemory {
    /// Build a recalled memory from a hybrid search result with inferred reason.
    pub fn from_scored(scored: ScoredMemory) -> Self {
        let reason = infer_recall_reason(&scored);
        Self {
            item: scored.item,
            reason,
            score_breakdown: scored.breakdown,
            sources: scored.sources,
        }
    }
}

/// Convert hybrid search results into explainable recalled memories.
pub fn explain_scored_memories(scored: Vec<ScoredMemory>) -> Vec<RecalledMemory> {
    scored
        .into_iter()
        .map(RecalledMemory::from_scored)
        .collect()
}

/// Infer the primary recall reason from hybrid search signals.
pub fn infer_recall_reason(scored: &ScoredMemory) -> RecallReason {
    let item = &scored.item;
    let breakdown = &scored.breakdown;
    let sources = &scored.sources;

    if sources.contains(&MemoryCandidateSource::Commitment) || item.kind == MemoryKind::Commitment {
        return RecallReason::ActivePromise;
    }

    if item.source == MemorySource::Ccv3 {
        return RecallReason::CharacterLore;
    }

    if matches!(item.kind, MemoryKind::Preference | MemoryKind::UserProfile) {
        return RecallReason::UserPreference;
    }

    if item.kind == MemoryKind::Affective
        || breakdown.emotional_match >= EMOTIONAL_MATCH_REASON_THRESHOLD
    {
        return RecallReason::EmotionalContinuity;
    }

    if sources.contains(&MemoryCandidateSource::Recent) || item.kind == MemoryKind::Episodic {
        return RecallReason::RecentConversation;
    }

    RecallReason::SimilarTopic
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ene_store::{AffectAnnotation, MemoryConfidence, MemorySalience, MemoryStatus};

    fn sample_item(kind: MemoryKind, source: MemorySource) -> MemoryItem {
        MemoryItem {
            id: Some(1),
            scope: ene_store::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind,
            title: "sample".into(),
            content: "sample content".into(),
            source,
            source_ref: None,
            confidence: MemoryConfidence::default(),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    fn sample_breakdown(emotional_match: f32) -> MemoryScoreBreakdown {
        MemoryScoreBreakdown {
            vector_similarity: 0.5,
            lexical_score: 0.1,
            recency_score: 0.2,
            salience: 0.5,
            confidence: 0.5,
            emotional_match,
            relationship: 0.0,
            access_boost: 0.0,
            relevance: 0.5,
            quality_factor: 1.0,
            contradiction_penalty: 0.0,
            stale_penalty: 0.0,
            commitment_boost: 0.0,
            total: 0.5,
        }
    }

    fn scored(
        item: MemoryItem,
        breakdown: MemoryScoreBreakdown,
        sources: Vec<MemoryCandidateSource>,
    ) -> ScoredMemory {
        ScoredMemory {
            item,
            breakdown,
            sources,
        }
    }

    #[test]
    fn commitment_source_maps_to_active_promise() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Inferred);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Commitment],
        ));
        assert_eq!(result, RecallReason::ActivePromise);
    }

    #[test]
    fn commitment_kind_maps_to_active_promise() {
        let item = sample_item(MemoryKind::Commitment, MemorySource::Inferred);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::ActivePromise);
    }

    #[test]
    fn ccv3_source_maps_to_character_lore() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Ccv3);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::CharacterLore);
    }

    #[test]
    fn preference_kind_maps_to_user_preference() {
        let item = sample_item(MemoryKind::Preference, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::UserPreference);
    }

    #[test]
    fn user_profile_kind_maps_to_user_preference() {
        let item = sample_item(MemoryKind::UserProfile, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::UserPreference);
    }

    #[test]
    fn affective_kind_maps_to_emotional_continuity() {
        let item = sample_item(MemoryKind::Affective, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::EmotionalContinuity);
    }

    #[test]
    fn high_emotional_match_score_maps_to_emotional_continuity() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(EMOTIONAL_MATCH_REASON_THRESHOLD),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::EmotionalContinuity);
    }

    #[test]
    fn moderate_emotional_match_does_not_override_similar_topic() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.3),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::SimilarTopic);
    }

    #[test]
    fn ccv3_preference_kind_maps_to_character_lore() {
        let item = sample_item(MemoryKind::Preference, MemorySource::Ccv3);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::CharacterLore);
    }

    #[test]
    fn recent_with_high_emotional_match_maps_to_emotional_continuity() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(EMOTIONAL_MATCH_REASON_THRESHOLD),
            vec![MemoryCandidateSource::Recent],
        ));
        assert_eq!(result, RecallReason::EmotionalContinuity);
    }

    #[test]
    fn recent_source_maps_to_recent_conversation() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Recent],
        ));
        assert_eq!(result, RecallReason::RecentConversation);
    }

    #[test]
    fn episodic_kind_maps_to_recent_conversation() {
        let item = sample_item(MemoryKind::Episodic, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::RecentConversation);
    }

    #[test]
    fn vector_only_fallback_maps_to_similar_topic() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Conversation);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        ));
        assert_eq!(result, RecallReason::SimilarTopic);
    }

    #[test]
    fn commitment_takes_priority_over_ccv3() {
        let item = sample_item(MemoryKind::Commitment, MemorySource::Ccv3);
        let result = infer_recall_reason(&scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Commitment],
        ));
        assert_eq!(result, RecallReason::ActivePromise);
    }

    #[test]
    fn from_scored_populates_all_fields() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Conversation);
        let breakdown = sample_breakdown(0.0);
        let sources = vec![
            MemoryCandidateSource::Vector,
            MemoryCandidateSource::Lexical,
        ];
        let scored = scored(item.clone(), breakdown.clone(), sources.clone());

        let recalled = RecalledMemory::from_scored(scored);
        assert_eq!(recalled.item.title, item.title);
        assert_eq!(recalled.reason, RecallReason::SimilarTopic);
        assert_eq!(recalled.score_breakdown, breakdown);
        assert_eq!(recalled.sources, sources);
    }

    #[test]
    fn explain_scored_memories_converts_batch() {
        let item = sample_item(MemoryKind::Preference, MemorySource::Conversation);
        let batch = vec![scored(
            item,
            sample_breakdown(0.0),
            vec![MemoryCandidateSource::Vector],
        )];
        let explained = explain_scored_memories(batch);
        assert_eq!(explained.len(), 1);
        assert_eq!(explained[0].reason, RecallReason::UserPreference);
    }

    #[test]
    fn recall_reason_serde_roundtrip() {
        let reasons = [
            RecallReason::SimilarTopic,
            RecallReason::RecentConversation,
            RecallReason::ActivePromise,
            RecallReason::CharacterLore,
            RecallReason::UserPreference,
            RecallReason::EmotionalContinuity,
            RecallReason::Pinned,
        ];
        for reason in reasons {
            let json = serde_json::to_string(&reason).unwrap();
            let back: RecallReason = serde_json::from_str(&json).unwrap();
            assert_eq!(reason, back);
        }
    }

    #[test]
    fn recalled_memory_serde_roundtrip() {
        let item = sample_item(MemoryKind::Semantic, MemorySource::Ccv3);
        let recalled = RecalledMemory {
            item,
            reason: RecallReason::CharacterLore,
            score_breakdown: sample_breakdown(0.0),
            sources: vec![MemoryCandidateSource::Vector],
        };
        let json = serde_json::to_string(&recalled).unwrap();
        let back: RecalledMemory = serde_json::from_str(&json).unwrap();
        assert_eq!(recalled, back);
    }
}
