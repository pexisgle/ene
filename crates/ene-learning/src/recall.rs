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

#[cfg(test)]
mod tests {
    use ene_primitive::RawId;

    use crate::recall::{RecallQuery, recall};
    use crate::test_support::{
        FakeLearningRepository, forget_memory, seed_memory, seed_memory_with_importance,
    };

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
    async fn importance_breaks_ties_between_equally_relevant_memories() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        // Equal term overlap: the older memory is deliberately the more
        // important one, so recency alone must not decide the order.
        let _ = seed_memory_with_importance(
            &repository,
            companion,
            "the owner likes tea",
            crate::Importance::clamped(5),
        )
        .await;
        let _ = seed_memory_with_importance(
            &repository,
            companion,
            "the owner likes coffee",
            crate::Importance::clamped(1),
        )
        .await;
        let recalled = recall(&repository, query(companion, "the owner likes", 2))
            .await
            .unwrap();
        assert_eq!(recalled.len(), 2);
        assert!(
            recalled[0].content.contains("tea"),
            "equal overlap must order by importance before recency, got {:?}",
            recalled[0].content
        );
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
                claim: None,
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
    async fn non_ascii_whole_runs_do_not_spend_a_lexical_slot() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let _ = recall(
            &repository,
            query(companion, "緑茶について教えてください", 5),
        )
        .await
        .unwrap();
        let calls = repository.recall_calls();
        let predicate = &calls[0].0;
        // The long run's bigrams already cover every row the run can match, so
        // the predicate must drop the run and spend all eight slots on terms
        // that can select rows.
        assert_eq!(
            predicate.len(),
            super::RECALL_MAX_TERMS,
            "the freed slot carries a bigram the run would evict: {predicate:?}"
        );
        assert!(
            predicate
                .iter()
                .all(|term| term.is_ascii() || term.chars().count() <= 2),
            "a non-ASCII whole run must not occupy a predicate slot: {predicate:?}"
        );
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
            id: crate::MemoryId::generate(),
            content: String::from("probe-recall-content"),
        };
        let rendered = format!("{memory:?}");
        assert!(!rendered.contains("probe-recall-content"));
    }
}
