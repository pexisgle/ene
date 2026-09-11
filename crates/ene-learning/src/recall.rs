//! Bounded recall of current Memory for one use.
//!
//! Recall is derived, not authoritative: it ranks the current durable
//! recognition for a query and never changes it. Normal forgetting suppresses
//! recall here; suppressed content stays in storage and can be recalled again
//! after a later change clears the suppression. Candidate retrieval is
//! bounded and multi-arm (newest, most important, and lexical matches), so an
//! old relevant Memory is not permanently excluded by a newest-rows window;
//! scoring stays term overlap, then importance, then recency, and needs no
//! embedding or index.

use ene_primitive::RawId;

use crate::repository::{LearningRepository, LearningTechnicalError};

/// Rows one candidate arm contributes to one recall.
///
/// Three arms run in one bounded query, so one recall decodes at most
/// `3 * RECALL_CANDIDATE_LIMIT` rows regardless of how many memories exist.
pub const RECALL_CANDIDATE_LIMIT: u64 = 200;

/// Query terms the lexical arm uses, longest first.
///
/// The cap keeps the generated `instr` predicate a constant size; the
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
#[derive(Clone, PartialEq, Eq)]
pub struct RecalledMemory {
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
/// rows, the most important rows, and rows matching the longest query
/// terms, so an old relevant Memory stays reachable after any number of
/// newer memories. An empty query still returns the highest importance,
/// newest memories so a caller can supply generic background. Equal scores
/// keep the repository's newest-first order.
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
            content: memory.content,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use ene_primitive::RawId;

    use crate::recall::{RecallQuery, recall};
    use crate::test_support::{FakeLearningRepository, forget_memory, seed_memory};

    fn query(companion: RawId, text: &str, limit: usize) -> RecallQuery {
        RecallQuery {
            companion,
            text: text.to_owned(),
            limit,
        }
    }

    #[tokio::test]
    async fn the_relevant_memory_is_recalled_first() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let _ = seed_memory(&repository, companion, "The owner likes jasmine tea.").await;
        let _ = seed_memory(&repository, companion, "The owner has a dog named Momo.").await;
        let recalled = recall(
            &repository,
            query(companion, "Which tea does the owner like?", 2),
        )
        .await
        .unwrap();
        assert_eq!(recalled.len(), 2);
        assert!(
            recalled[0].content.contains("jasmine tea"),
            "the overlapping memory ranks first, got {:?}",
            recalled[0].content
        );
    }

    #[tokio::test]
    async fn suppressed_memories_are_not_recalled() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let (memory, revision) =
            seed_memory(&repository, companion, "The owner likes jasmine tea.").await;
        let _ = seed_memory(&repository, companion, "The owner has a dog named Momo.").await;
        forget_memory(
            &repository,
            companion,
            memory,
            revision,
            "The owner likes jasmine tea.",
        )
        .await;
        let recalled = recall(&repository, query(companion, "jasmine tea preference", 10))
            .await
            .unwrap();
        assert!(
            recalled
                .iter()
                .all(|memory| !memory.content.contains("jasmine tea")),
            "suppressed content stays stored but is not recalled"
        );
    }

    #[tokio::test]
    async fn another_companions_memories_are_never_recalled() {
        let owner = RawId::new();
        let other = RawId::new();
        let repository = FakeLearningRepository::new();
        let _ = seed_memory(&repository, other, "The owner likes jasmine tea.").await;
        let recalled = recall(&repository, query(owner, "jasmine tea", 10))
            .await
            .unwrap();
        assert!(
            recalled.is_empty(),
            "recall is scoped to the query companion"
        );
    }

    #[tokio::test]
    async fn the_limit_bounds_the_returned_window() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        for content in ["owner likes tea", "owner likes coffee", "owner likes water"] {
            let _ = seed_memory(&repository, companion, content).await;
        }
        let recalled = recall(&repository, query(companion, "owner likes", 2))
            .await
            .unwrap();
        assert_eq!(recalled.len(), 2);
    }

    #[tokio::test]
    async fn japanese_queries_match_by_character_overlap() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let _ = seed_memory(&repository, companion, "owner は緑茶が好き").await;
        let recalled = recall(&repository, query(companion, "緑茶について教えて", 1))
            .await
            .unwrap();
        assert_eq!(recalled.len(), 1);
        assert!(recalled[0].content.contains("緑茶"));
    }

    #[tokio::test]
    async fn an_old_memory_is_reachable_past_the_newest_window() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let _ = seed_memory(&repository, companion, "The owner likes jasmine tea.").await;
        for index in 0..250 {
            let _ = seed_memory(&repository, companion, &format!("filler memory {index}")).await;
        }
        let recalled = recall(
            &repository,
            query(companion, "Which tea does the owner like?", 1),
        )
        .await
        .unwrap();
        assert_eq!(recalled.len(), 1);
        assert!(
            recalled[0].content.contains("jasmine tea"),
            "the oldest relevant memory must stay reachable: {recalled:?}"
        );
    }

    #[tokio::test]
    async fn suppressed_newest_rows_do_not_consume_candidate_slots() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let _ = seed_memory(&repository, companion, "The owner likes jasmine tea.").await;
        for index in 0..250 {
            let content = format!("latest memory {index}");
            let (memory, revision) = seed_memory(&repository, companion, &content).await;
            forget_memory(&repository, companion, memory, revision, &content).await;
        }
        let recalled = recall(&repository, query(companion, "jasmine tea", 1))
            .await
            .unwrap();
        assert_eq!(
            recalled.len(),
            1,
            "suppressed rows must not crowd out the active candidate"
        );
        assert!(recalled[0].content.contains("jasmine tea"));
    }

    #[tokio::test]
    async fn an_updated_old_memory_is_a_candidate_with_current_content() {
        use crate::repository::{
            LearningRepository as _, MemoryChange, MemoryChangeCommit, MemoryChangeOutcome,
            MemoryTarget,
        };
        use crate::{ChangeKind, Importance, LearningScope, TemporalMeaning};

        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let (memory, revision) =
            seed_memory(&repository, companion, "The owner drinks something warm.").await;
        for index in 0..250 {
            let _ = seed_memory(&repository, companion, &format!("filler memory {index}")).await;
        }
        let outcome = repository
            .commit_memory_change(MemoryChangeCommit {
                summary: None,
                secret_premise: None,
                change: MemoryChange {
                    target: MemoryTarget::Existing {
                        id: memory,
                        expected_revision: revision,
                    },
                    scope: LearningScope::companion(companion),
                    content: String::from("The owner likes jasmine tea now."),
                    importance: Importance::default(),
                    temporal: TemporalMeaning::Enduring,
                    change: ChangeKind::Refined,
                    recall_suppressed: false,
                    at: ene_primitive::WallClockWithTz::now(),
                },
            })
            .await
            .unwrap();
        assert!(matches!(outcome, MemoryChangeOutcome::Committed { .. }));
        let recalled = recall(&repository, query(companion, "jasmine tea", 1))
            .await
            .unwrap();
        assert_eq!(recalled.len(), 1);
        assert!(recalled[0].content.contains("jasmine tea now"));
    }

    #[tokio::test]
    async fn recall_candidate_work_is_bounded() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let text = (0..50)
            .map(|index| format!("term{index}x"))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = recall(&repository, query(companion, &text, 5))
            .await
            .unwrap();
        let calls = repository.recall_calls();
        assert_eq!(calls.len(), 1, "one bounded candidate query per recall");
        assert!(
            calls[0].0.len() <= super::RECALL_MAX_TERMS,
            "the lexical arm stays a constant size, got {}",
            calls[0].0.len()
        );
        assert_eq!(calls[0].1, super::RECALL_CANDIDATE_LIMIT);
    }

    #[tokio::test]
    async fn an_empty_query_still_returns_the_newest_memories() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        for content in ["first", "second", "third"] {
            let _ = seed_memory(&repository, companion, content).await;
        }
        let recalled = recall(&repository, query(companion, "", 2)).await.unwrap();
        assert_eq!(recalled.len(), 2);
        assert_eq!(recalled[0].content, "third", "newest first");
        assert_eq!(recalled[1].content, "second");
    }

    #[test]
    fn recalled_memory_debug_redacts_content() {
        let memory = crate::recall::RecalledMemory {
            content: String::from("probe-recall-content"),
        };
        let rendered = format!("{memory:?}");
        assert!(!rendered.contains("probe-recall-content"));
    }
}
