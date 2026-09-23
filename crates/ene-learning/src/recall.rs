use ene_primitive::RawId;

use crate::identity::MemoryId;
use crate::repository::{LearningRepository, LearningTechnicalError};

const RECALL_CANDIDATE_LIMIT: u64 = 200;

const RECALL_MAX_TERMS: usize = 8;

#[derive(Clone, PartialEq, Eq)]
pub struct RecallQuery {
    pub companion: RawId,
    pub text: String,
    pub limit: usize,
}

impl core::fmt::Debug for RecallQuery {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RecallQuery")
            .field("companion", &self.companion)
            .field("text", &"[redacted]")
            .field("limit", &self.limit)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RecalledMemory {
    pub id: MemoryId,
    pub content: String,
}

impl core::fmt::Debug for RecalledMemory {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RecalledMemory")
            .field("content", &"[redacted]")
            .finish()
    }
}

pub async fn recall(
    repository: &impl LearningRepository,
    query: RecallQuery,
) -> Result<Vec<RecalledMemory>, LearningTechnicalError> {
    let mut terms = crate::relevance::recall_index_terms(&query.text);
    terms.sort_by(|left, right| {
        right
            .chars()
            .count()
            .cmp(&left.chars().count())
            .then(left.cmp(right))
    });
    let mut predicate: Vec<String> = terms
        .iter()
        .filter(|term| term.is_ascii() || term.chars().count() <= 2)
        .cloned()
        .collect();
    predicate.truncate(RECALL_MAX_TERMS);
    terms.truncate(RECALL_MAX_TERMS);
    let memories = repository
        .recall_candidates(query.companion, &predicate, RECALL_CANDIDATE_LIMIT)
        .await?;
    let mut ranked: Vec<(usize, crate::memory::Memory)> = memories
        .into_iter()
        .map(|memory| (crate::relevance::overlap(&terms, &memory.content), memory))
        .collect();
    ranked.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then(right.importance.cmp(&left.importance))
    });
    Ok(ranked
        .into_iter()
        .take(query.limit)
        .map(|(_, memory)| RecalledMemory {
            id: memory.id,
            content: memory.content,
        })
        .collect())
}
