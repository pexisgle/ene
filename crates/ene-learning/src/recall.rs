use ene_primitive::RawId;

use crate::identity::MemoryId;
use crate::repository::{LearningRepository, LearningTechnicalError};

pub const RECALL_CANDIDATE_LIMIT: u64 = 200;

const RECALL_MAX_TERMS: usize = 8;

/// A query for the Memory one use can draw on.
#[derive(Clone, PartialEq, Eq)]
pub struct RecallQuery {
    pub companion: RawId,
    /// Text the recall is for, typically the current owner input; redacted
    /// from [`core::fmt::Debug`].
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

/// One recalled Memory, projected for use in a context.
///
/// The identity travels with the content: a caller that puts the content into
/// a logical input can name the canonical Memory it consumed, so a deletion
/// admission can associate the use with the interval its provenance belongs
/// to (`erasure_use_hold`). It is an opaque correlation, never a body.
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
    // Longest first: the cap keeps the generated lexical predicate bounded,
    // and a longer term is stronger evidence than a short bigram. Ties sort
    // by the term itself so the query stays deterministic.
    terms.sort_by(|left, right| {
        right
            .chars()
            .count()
            .cmp(&left.chars().count())
            .then(left.cmp(right))
    });
    // The durable index stores every bigram of a non-ASCII run alongside the
    // run itself, so a whole run can never match a content row its bigrams do
    // not. Dropping whole runs from the predicate frees cap slots for bigrams
    // the run would otherwise evict; scoring below keeps the runs, where an
    // exact-run hit is stronger evidence than a lone bigram.
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
