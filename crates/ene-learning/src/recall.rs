//! Bounded recall of current Memory for one use.
//!
//! Recall is derived, not authoritative: it ranks the current durable
//! recognition for a query and never changes it. Normal forgetting suppresses
//! recall here; suppressed content stays in storage and can be recalled again
//! after a later change clears the suppression. Candidate retrieval is
//! bounded and multi-arm (newest, most important, and lexical matches), so an
//! old relevant Memory is not permanently excluded by a newest-rows window;
//! scoring stays term overlap, then importance, then recency, and needs no
//! embedding. The lexical arm matches query terms against a derived token
//! index rather than scanning stored content.

use ene_primitive::RawId;

use crate::identity::MemoryId;
use crate::repository::{LearningRepository, LearningTechnicalError};

/// Rows one candidate arm contributes to one recall.
///
/// Three arms run in one bounded, index-backed query, so one recall decodes
/// at most `3 * RECALL_CANDIDATE_LIMIT` rows regardless of how many memories
/// exist, and finding those rows visits at most one index walk of `limit`
/// entries per arm rather than scanning the companion's whole set.
pub const RECALL_CANDIDATE_LIMIT: u64 = 200;

/// Query terms the lexical arm uses, longest first.
///
/// The cap keeps the generated token predicate a constant size; the
/// longest terms are the most selective lexical evidence.
const RECALL_MAX_TERMS: usize = 8;

/// A query for the Memory one use can draw on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallQuery {
    /// Companion whose scope is searched; no other scope is consulted.
    pub companion: RawId,
    /// Text the recall is for, typically the current owner input.
    pub text: String,
    /// Maximum number of memories returned.
    pub limit: usize,
}

/// One recalled Memory, projected for use in a context.
///
/// The identity travels with the content: a caller that puts the content into
/// a logical input can name the canonical Memory it consumed, so a deletion
/// admission can associate the use with the interval its provenance belongs
/// to (`erasure_use_hold`). It is an opaque correlation, never a body.
#[derive(Clone, PartialEq, Eq)]
pub struct RecalledMemory {
    /// Canonical identity of the Memory this content is the current
    /// recognition of.
    pub id: MemoryId,
    /// Recognition text; redacted from [`core::fmt::Debug`].
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

/// Ranks the current companion-scoped memories for `query`.
///
/// Suppressed memories are excluded by candidate retrieval, before any cap
/// applies. Candidates come from one bounded multi-arm query: the newest
/// rows, the most important rows, and rows whose derived index tokens carry
/// the longest query terms, so an old relevant Memory stays reachable after
/// any number of newer memories. An empty query still returns the highest
/// importance, newest memories so a caller can supply generic background.
/// Equal scores keep the repository's newest-first order.
///
/// # Errors
///
/// [`LearningTechnicalError`] reports storage failure. An empty result means
/// no available Memory, never an error.
pub async fn recall(
    repository: &impl LearningRepository,
    query: RecallQuery,
) -> Result<Vec<RecalledMemory>, LearningTechnicalError> {
    let mut terms = crate::relevance::terms(&query.text);
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
    terms.truncate(RECALL_MAX_TERMS);
    let memories = repository
        .recall_candidates(query.companion, &terms, RECALL_CANDIDATE_LIMIT)
        .await?;
    let mut ranked: Vec<(usize, crate::memory::Memory)> = memories
        .into_iter()
        .map(|memory| (crate::relevance::overlap(&terms, &memory.content), memory))
        .collect();
    // Stable sort: equal scores keep the repository's newest-first order, so
    // importance breaks ties before recency.
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
