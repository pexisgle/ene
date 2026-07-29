//! Maps hybrid search output from `ene-store` into explainable recall results.

use ene_core::ScoredMemory;

use super::result::{RecalledMemory, explain_scored_memories};

/// Maps hybrid search results into explainable recalled memories.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecallResultMapper;

impl RecallResultMapper {
    /// Convert hybrid search results into explainable recalled memories.
    pub fn map(scored: Vec<ScoredMemory>) -> Vec<RecalledMemory> {
        explain_scored_memories(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemorySource, MemoryStatus,
    };

    fn sample_scored() -> ScoredMemory {
        ScoredMemory {
            item: MemoryItem {
                id: Some(1),
                scope: MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: MemoryKind::Semantic,
                title: "sample".into(),
                content: "sample content".into(),
                source: MemorySource::Conversation,
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
            },
            breakdown: ene_store::MemoryScoreBreakdown {
                vector_similarity: 0.5,
                lexical_score: 0.1,
                recency_score: 0.2,
                salience: 0.5,
                confidence: 0.5,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                total: 0.5,
            },
            sources: vec![ene_store::MemoryCandidateSource::Vector],
        }
    }

    #[test]
    fn mapper_delegates_to_explain_scored_memories() {
        let explained = RecallResultMapper::map(vec![sample_scored()]);
        assert_eq!(explained.len(), 1);
        assert_eq!(
            explained[0].reason,
            super::super::result::RecallReason::SimilarTopic
        );
    }
}
