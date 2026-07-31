//! Prompt qualifiers for recalled typed memories (#76).

use ene_core::MemoryStatus;

use crate::recall::RecalledMemory;

/// Prefix recalled memory content for prompt injection when uncertainty applies.
pub fn format_recalled_content(memory: &RecalledMemory) -> String {
    let qualifier = recall_content_qualifier(memory);
    match qualifier {
        Some(prefix) => format!("{prefix}{}", memory.item.content),
        None => memory.item.content.clone(),
    }
}

/// Return a prompt prefix for uncertain or disputed memories, if any.
pub fn recall_content_qualifier(memory: &RecalledMemory) -> Option<&'static str> {
    if memory.item.status == MemoryStatus::Faded {
        return Some("[uncertain] ");
    }
    if memory.item.status == MemoryStatus::Disputed {
        return Some("[disputed] ");
    }
    if memory.item.status == MemoryStatus::Active && memory.item.confidence.get() < 0.5 {
        return Some("[uncertain] ");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };

    use crate::recall::{RecallReason, RecalledMemory};

    fn sample_recalled(status: MemoryStatus, confidence: f32) -> RecalledMemory {
        RecalledMemory {
            item: MemoryItem {
                id: Some(1),
                scope: MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: MemoryKind::Semantic,
                title: "fact".into(),
                content: "likes tea".into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::new(confidence),
                salience: MemorySalience::default(),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                valid_from: None,
                valid_until: None,
                status,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: None,
            },
            reason: RecallReason::SimilarTopic,
            score_breakdown: MemoryScoreBreakdown {
                vector_similarity: 0.0,
                lexical_score: 0.0,
                recency_score: 0.0,
                salience: 0.0,
                confidence: 0.0,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                relevance: 0.0,
                quality_factor: 1.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                total: 0.0,
            },
            sources: vec![],
        }
    }

    #[test]
    fn faded_memory_gets_uncertain_prefix() {
        let memory = sample_recalled(MemoryStatus::Faded, 0.9);
        assert_eq!(format_recalled_content(&memory), "[uncertain] likes tea");
    }

    #[test]
    fn disputed_memory_gets_disputed_prefix() {
        let memory = sample_recalled(MemoryStatus::Disputed, 0.9);
        assert_eq!(format_recalled_content(&memory), "[disputed] likes tea");
    }

    #[test]
    fn low_confidence_active_memory_gets_uncertain_prefix() {
        let memory = sample_recalled(MemoryStatus::Active, 0.3);
        assert_eq!(format_recalled_content(&memory), "[uncertain] likes tea");
    }

    #[test]
    fn active_high_confidence_has_no_prefix() {
        let memory = sample_recalled(MemoryStatus::Active, 0.9);
        assert_eq!(format_recalled_content(&memory), "likes tea");
    }
}
