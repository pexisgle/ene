//! MMR diversification for hybrid recall candidates (#78).
//!
//! Downstream recall execution calls [`MemoryDiversifyPipeline::diversify`] after
//! `MemoryStore::search`.
//!
//! The pipeline merges near-duplicate clusters, applies greedy MMR selection,
//! enforces per-kind minimum slots, and rewards source diversity. Hybrid scores
//! on each [`ScoredMemory`] are preserved unchanged.

use ene_core::{MemoryCandidateSource, MemoryKind, ScoredMemory};

use crate::config::MindMemoryConfig;

use super::plan::RecallPlan;

/// Runtime options controlling MMR diversification behavior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryDiversifyOptions {
    /// MMR relevance-vs-diversity tradeoff in `[0.0, 1.0]`.
    pub lambda: f32,
    /// Lexical similarity threshold for duplicate cluster merging.
    pub duplicate_cluster_threshold: f32,
    /// Minimum slots reserved for semantic memories.
    pub min_slots_semantic: usize,
    /// Minimum slots reserved for episodic memories.
    pub min_slots_episodic: usize,
    /// Minimum slots reserved for user profile memories.
    pub min_slots_user_profile: usize,
    /// Minimum slots reserved for commitment memories.
    pub min_slots_commitment: usize,
    /// Bonus added when a candidate introduces a new recall source type.
    pub source_diversity_bonus: f32,
}

impl MemoryDiversifyOptions {
    /// Build options from mind memory config.
    pub const fn from_config(config: &MindMemoryConfig) -> Self {
        Self {
            lambda: config.mmr_lambda.clamp(0.0, 1.0),
            duplicate_cluster_threshold: config.mmr_duplicate_cluster_threshold.clamp(0.0, 1.0),
            min_slots_semantic: config.mmr_min_slots_semantic,
            min_slots_episodic: config.mmr_min_slots_episodic,
            min_slots_user_profile: config.mmr_min_slots_user_profile,
            min_slots_commitment: config.mmr_min_slots_commitment,
            source_diversity_bonus: config.mmr_source_diversity_bonus.clamp(0.0, 1.0),
        }
    }
}

/// Deterministic MMR diversification pipeline for recall candidates.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryDiversifyPipeline;

impl MemoryDiversifyPipeline {
    /// Diversify hybrid-search candidates using MMR and kind quotas.
    pub fn diversify(
        candidates: Vec<ScoredMemory>,
        plan: &RecallPlan,
        options: MemoryDiversifyOptions,
    ) -> Vec<ScoredMemory> {
        let limit = plan.budget.result_limit.max(1);

        if candidates.is_empty() {
            return candidates;
        }

        if candidates.len() <= 1 {
            return truncate(candidates, limit);
        }

        let input_count = candidates.len();
        let clustered = cluster_dedup(candidates, options.duplicate_cluster_threshold);
        let clusters_merged = input_count.saturating_sub(clustered.len());

        let mut selected = greedy_mmr(&clustered, limit, options);
        apply_kind_quotas(&mut selected, &clustered, plan, options, limit);

        tracing::debug!(
            component = "Recall",
            stage = "diversify",
            outcome = "completed",
            input_count,
            pool_count = clustered.len(),
            output_count = selected.len(),
            clusters_merged,
            kind_distribution = ?kind_counts(&selected),
        );

        selected
    }
}

fn truncate(mut candidates: Vec<ScoredMemory>, limit: usize) -> Vec<ScoredMemory> {
    candidates.truncate(limit);
    candidates
}

/// Jaccard similarity between two memory documents (title + content tokens).
///
/// Inlined from `ene-store`'s `search::document_lexical_similarity` (#309)
/// so `ene-mind` no longer needs a production dependency on `ene-store`.
/// A future `ene-rag` crate (#302) will own this properly.
fn document_lexical_similarity(
    title_a: &str,
    content_a: &str,
    title_b: &str,
    content_b: &str,
) -> f32 {
    use std::collections::HashSet;

    fn tokenize(text: &str) -> HashSet<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    }

    let tokens_a: HashSet<String> = tokenize(title_a)
        .into_iter()
        .chain(tokenize(content_a))
        .collect();
    let tokens_b: HashSet<String> = tokenize(title_b)
        .into_iter()
        .chain(tokenize(content_b))
        .collect();
    if tokens_a.is_empty() || tokens_b.is_empty() {
        return 0.0;
    }
    let intersection = tokens_a.intersection(&tokens_b).count();
    let union = tokens_a.union(&tokens_b).count();
    if union == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts are small; f32 precision is sufficient for similarity scoring"
    )]
    let ratio = intersection as f32 / union as f32;
    ratio
}

fn item_similarity(a: &ScoredMemory, b: &ScoredMemory) -> f32 {
    document_lexical_similarity(
        &a.item.title,
        &a.item.content,
        &b.item.title,
        &b.item.content,
    )
}

/// Merge near-duplicate candidates, keeping the highest-scoring representative.
fn cluster_dedup(mut candidates: Vec<ScoredMemory>, threshold: f32) -> Vec<ScoredMemory> {
    candidates.sort_by(|a, b| {
        b.breakdown
            .total
            .partial_cmp(&a.breakdown.total)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut representatives: Vec<ScoredMemory> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let is_duplicate = representatives
            .iter()
            .any(|rep| item_similarity(&candidate, rep) >= threshold);
        if !is_duplicate {
            representatives.push(candidate);
        }
    }

    representatives
}

fn greedy_mmr(
    pool: &[ScoredMemory],
    limit: usize,
    options: MemoryDiversifyOptions,
) -> Vec<ScoredMemory> {
    let max_relevance = pool
        .iter()
        .map(|c| c.breakdown.total)
        .fold(0.0_f32, f32::max)
        .max(f32::EPSILON);

    let mut selected: Vec<ScoredMemory> = Vec::with_capacity(limit.min(pool.len()));
    let mut remaining: Vec<ScoredMemory> = pool.to_vec();

    while selected.len() < limit {
        let Some((best_idx, _)) = remaining
            .iter()
            .enumerate()
            .map(|(idx, candidate)| {
                let mmr_score = mmr_score(candidate, &selected, options, max_relevance);
                (idx, mmr_score)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        else {
            break;
        };

        let chosen = remaining.remove(best_idx);
        selected.push(chosen);
    }

    selected
}

fn mmr_score(
    candidate: &ScoredMemory,
    selected: &[ScoredMemory],
    options: MemoryDiversifyOptions,
    max_relevance: f32,
) -> f32 {
    let relevance = candidate.breakdown.total / max_relevance;
    let max_sim = if selected.is_empty() {
        0.0
    } else {
        selected
            .iter()
            .map(|s| item_similarity(candidate, s))
            .fold(0.0_f32, f32::max)
    };
    let source_bonus = source_diversity_bonus(candidate, selected, options.source_diversity_bonus);
    (1.0 - options.lambda).mul_add(-max_sim, options.lambda * relevance) + source_bonus
}

fn source_diversity_bonus(candidate: &ScoredMemory, selected: &[ScoredMemory], bonus: f32) -> f32 {
    if bonus <= 0.0 || selected.is_empty() {
        return 0.0;
    }

    let selected_sources: std::collections::HashSet<MemoryCandidateSource> = selected
        .iter()
        .flat_map(|m| m.sources.iter().copied())
        .collect();

    let introduces_new = candidate
        .sources
        .iter()
        .any(|source| !selected_sources.contains(source));

    if introduces_new { bonus } else { 0.0 }
}

fn effective_min_slots(
    plan: &RecallPlan,
    options: MemoryDiversifyOptions,
    limit: usize,
) -> Vec<(MemoryKind, usize)> {
    let mut requested: std::collections::HashMap<MemoryKind, usize> =
        std::collections::HashMap::from([
            (MemoryKind::Semantic, options.min_slots_semantic),
            (MemoryKind::Episodic, options.min_slots_episodic),
            (MemoryKind::UserProfile, options.min_slots_user_profile),
            (MemoryKind::Commitment, options.min_slots_commitment),
        ]);

    for kind in &plan.required_kinds {
        requested
            .entry(*kind)
            .and_modify(|count| *count = (*count).max(1))
            .or_insert(1);
    }

    let total_min: usize = requested.values().sum();
    if total_min > limit {
        tracing::warn!(
            component = "Recall",
            stage = "diversify",
            total_min,
            result_limit = limit,
            "Kind minimum slots exceed result limit; allocating by priority"
        );
    }

    // Highest-priority kinds are allocated first when the budget is tight.
    const KIND_QUOTA_PRIORITY: [MemoryKind; 9] = [
        MemoryKind::Commitment,
        MemoryKind::UserProfile,
        MemoryKind::Preference,
        MemoryKind::Semantic,
        MemoryKind::Episodic,
        MemoryKind::Relationship,
        MemoryKind::Affective,
        MemoryKind::Procedure,
        MemoryKind::Reflection,
    ];

    let mut allocated = Vec::new();
    let mut remaining = limit;
    for kind in KIND_QUOTA_PRIORITY {
        let desired = requested.get(&kind).copied().unwrap_or(0);
        if desired == 0 || remaining == 0 {
            continue;
        }
        let slots = desired.min(remaining);
        allocated.push((kind, slots));
        remaining -= slots;
    }

    allocated
}

fn apply_kind_quotas(
    selected: &mut Vec<ScoredMemory>,
    pool: &[ScoredMemory],
    plan: &RecallPlan,
    options: MemoryDiversifyOptions,
    limit: usize,
) {
    let mins = effective_min_slots(plan, options, limit);

    for &(kind, min_count) in &mins {
        let current = selected.iter().filter(|m| m.item.kind == kind).count();
        if current >= min_count {
            continue;
        }

        let deficit = min_count - current;
        for _ in 0..deficit {
            let Some(replacement) = best_pool_candidate_for_kind(pool, selected, kind) else {
                break;
            };

            let swap_idx = lowest_scoring_swappable_index(selected, &mins);
            let Some(swap_idx) = swap_idx else {
                if selected.len() < limit {
                    selected.push(replacement);
                }
                continue;
            };

            selected[swap_idx] = replacement;
        }
    }
}

fn best_pool_candidate_for_kind(
    pool: &[ScoredMemory],
    selected: &[ScoredMemory],
    kind: MemoryKind,
) -> Option<ScoredMemory> {
    pool.iter()
        .filter(|c| c.item.kind == kind)
        .filter(|c| {
            let id = c.item.id;
            !selected.iter().any(|s| s.item.id == id)
        })
        .max_by(|a, b| {
            a.breakdown
                .total
                .partial_cmp(&b.breakdown.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

fn lowest_scoring_swappable_index(
    selected: &[ScoredMemory],
    mins: &[(MemoryKind, usize)],
) -> Option<usize> {
    let min_for = |kind: MemoryKind| -> usize {
        mins.iter().find(|(k, _)| *k == kind).map_or(0, |(_, n)| *n)
    };

    let mut kind_counts: std::collections::HashMap<MemoryKind, usize> =
        std::collections::HashMap::new();
    for memory in selected {
        *kind_counts.entry(memory.item.kind).or_insert(0) += 1;
    }

    selected
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            let count = kind_counts.get(&m.item.kind).copied().unwrap_or(0);
            count > min_for(m.item.kind)
        })
        .min_by(|(_, a), (_, b)| {
            a.breakdown
                .total
                .partial_cmp(&b.breakdown.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(idx, _)| idx)
}

fn kind_counts(selected: &[ScoredMemory]) -> std::collections::HashMap<&'static str, usize> {
    let mut counts = std::collections::HashMap::new();
    for memory in selected {
        *counts.entry(memory.item.kind.as_str()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
#[cfg_attr(
    test,
    expect(clippy::float_cmp, reason = "test asserts exact float equality")
)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };

    use crate::recall::{RecallBudgetHints, RecallScopeFilter, RecallSearchHints};

    fn sample_breakdown(total: f32) -> MemoryScoreBreakdown {
        MemoryScoreBreakdown {
            vector_similarity: total,
            lexical_score: 0.0,
            recency_score: 0.0,
            salience: 0.0,
            confidence: 0.0,
            emotional_match: 0.0,
            relationship: 0.0,
            access_boost: 0.0,
            contradiction_penalty: 0.0,
            stale_penalty: 0.0,
            commitment_boost: 0.0,
            total,
        }
    }

    fn sample_memory(
        id: i64,
        kind: MemoryKind,
        title: &str,
        content: &str,
        total: f32,
    ) -> ScoredMemory {
        ScoredMemory {
            item: MemoryItem {
                id: Some(id),
                scope: MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind,
                title: title.into(),
                content: content.into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::default(),
                salience: MemorySalience::default(),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: chrono::Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
                updated_at: chrono::Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: None,
            },
            breakdown: sample_breakdown(total),
            sources: vec![MemoryCandidateSource::Vector],
        }
    }

    fn default_plan(limit: usize) -> RecallPlan {
        RecallPlan {
            current_topic: "test".into(),
            semantic_queries: vec!["test".into()],
            episodic_queries: vec![],
            required_kinds: vec![MemoryKind::Semantic, MemoryKind::Episodic],
            scope: RecallScopeFilter {
                character_id: "ene".into(),
                user_id: None,
            },
            budget: RecallBudgetHints {
                memory_budget_tokens: 1000,
                semantic_budget_tokens: 500,
                result_limit: limit,
            },
            search: RecallSearchHints {
                primary_query_text: "test".into(),
                similarity_threshold: 0.35,
                min_score: 0.2,
                decay_half_life_days: 30.0,
                query_affect: None,
            },
        }
    }

    fn default_options() -> MemoryDiversifyOptions {
        MemoryDiversifyOptions {
            lambda: 0.7,
            duplicate_cluster_threshold: 0.75,
            min_slots_semantic: 1,
            min_slots_episodic: 1,
            min_slots_user_profile: 1,
            min_slots_commitment: 1,
            source_diversity_bonus: 0.05,
        }
    }

    #[test]
    fn cluster_dedup_keeps_one_representative() {
        let candidates = vec![
            sample_memory(
                1,
                MemoryKind::Episodic,
                "pizza night",
                "We love pepperoni pizza every Friday",
                0.9,
            ),
            sample_memory(
                2,
                MemoryKind::Episodic,
                "pizza night",
                "We love pepperoni pizza every Friday",
                0.85,
            ),
            sample_memory(
                3,
                MemoryKind::Episodic,
                "pizza night",
                "We love pepperoni pizza every Friday",
                0.8,
            ),
            sample_memory(
                4,
                MemoryKind::Semantic,
                "weather",
                "It was sunny today",
                0.5,
            ),
        ];

        let merged = cluster_dedup(candidates, 0.75);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].item.id, Some(1));
    }

    #[test]
    fn mmr_diversifies_similar_episodic_memories() {
        let candidates = vec![
            sample_memory(
                1,
                MemoryKind::Episodic,
                "pizza a",
                "pepperoni pizza friday tradition family",
                0.95,
            ),
            sample_memory(
                2,
                MemoryKind::Episodic,
                "pizza b",
                "pepperoni pizza friday tradition family night",
                0.94,
            ),
            sample_memory(
                3,
                MemoryKind::Episodic,
                "pizza c",
                "pepperoni pizza friday tradition family dinner",
                0.93,
            ),
            sample_memory(
                4,
                MemoryKind::Episodic,
                "pizza d",
                "pepperoni pizza friday tradition family fun",
                0.92,
            ),
            sample_memory(
                5,
                MemoryKind::Episodic,
                "pizza e",
                "pepperoni pizza friday tradition family time",
                0.91,
            ),
            sample_memory(
                6,
                MemoryKind::Semantic,
                "programming",
                "Rust ownership and borrowing rules",
                0.4,
            ),
        ];

        let plan = default_plan(5);
        let options = MemoryDiversifyOptions {
            min_slots_semantic: 1,
            min_slots_episodic: 1,
            min_slots_user_profile: 0,
            min_slots_commitment: 0,
            ..default_options()
        };
        let result = MemoryDiversifyPipeline::diversify(candidates, &plan, options);

        assert_eq!(result.len(), 5);
        assert!(
            result.iter().any(|m| m.item.kind == MemoryKind::Semantic),
            "semantic memory should survive MMR diversification"
        );
    }

    #[test]
    fn commitment_min_slot_preserved_over_episodic_flood() {
        let mut episodic: Vec<ScoredMemory> = (1..=10)
            .map(|i| {
                sample_memory(
                    i,
                    MemoryKind::Episodic,
                    &format!("event {i}"),
                    &format!("Something happened on day {i} unrelated topic"),
                    (i as f32).mul_add(-0.01, 0.9),
                )
            })
            .collect();
        episodic.push(sample_memory(
            99,
            MemoryKind::Commitment,
            "follow up",
            "Promise to review the project proposal tomorrow",
            0.15,
        ));

        let plan = default_plan(5);
        let options = MemoryDiversifyOptions {
            min_slots_commitment: 1,
            min_slots_semantic: 0,
            min_slots_episodic: 0,
            min_slots_user_profile: 0,
            ..default_options()
        };
        let result = MemoryDiversifyPipeline::diversify(episodic, &plan, options);

        assert!(
            result.iter().any(|m| m.item.kind == MemoryKind::Commitment),
            "commitment should be preserved via minimum slot"
        );
    }

    #[test]
    fn user_profile_min_slot_preserved() {
        let candidates = vec![
            sample_memory(
                1,
                MemoryKind::Episodic,
                "event",
                "Went to the park today",
                0.95,
            ),
            sample_memory(
                2,
                MemoryKind::Episodic,
                "event2",
                "Visited the museum yesterday",
                0.94,
            ),
            sample_memory(
                3,
                MemoryKind::Episodic,
                "event3",
                "Had dinner with friends",
                0.93,
            ),
            sample_memory(
                4,
                MemoryKind::UserProfile,
                "name",
                "The user's name is Alex",
                0.2,
            ),
        ];

        let plan = default_plan(3);
        let result = MemoryDiversifyPipeline::diversify(candidates, &plan, default_options());

        assert!(
            result
                .iter()
                .any(|m| m.item.kind == MemoryKind::UserProfile),
            "user profile should be preserved via minimum slot"
        );
    }

    #[test]
    fn source_diversity_bonus_prefers_new_source() {
        let mut lexical = sample_memory(
            1,
            MemoryKind::Semantic,
            "topic",
            "shared content alpha",
            0.5,
        );
        lexical.sources = vec![MemoryCandidateSource::Lexical];
        let mut vector =
            sample_memory(2, MemoryKind::Semantic, "topic", "shared content beta", 0.5);
        vector.sources = vec![MemoryCandidateSource::Vector];

        let selected = vec![lexical.clone()];
        let bonus = source_diversity_bonus(&vector, &selected, 0.1);
        assert!(bonus > 0.0);

        let no_bonus = source_diversity_bonus(&lexical, &selected, 0.1);
        assert!(no_bonus < f32::EPSILON);
    }

    #[test]
    fn scores_are_not_modified() {
        let candidates = vec![
            sample_memory(1, MemoryKind::Semantic, "a", "unique alpha content", 0.9),
            sample_memory(2, MemoryKind::Episodic, "b", "unique beta content", 0.8),
        ];

        let plan = default_plan(2);
        let result = MemoryDiversifyPipeline::diversify(candidates, &plan, default_options());

        assert_eq!(result[0].breakdown.total, 0.9);
        assert_eq!(result[1].breakdown.total, 0.8);
    }

    #[test]
    fn options_from_config_uses_defaults() {
        let config = MindMemoryConfig::default();
        let options = MemoryDiversifyOptions::from_config(&config);
        assert!((options.lambda - 0.7).abs() < f32::EPSILON);
        assert_eq!(options.min_slots_commitment, 1);
    }

    #[test]
    fn single_candidate_passthrough() {
        let candidates = vec![sample_memory(
            1,
            MemoryKind::Semantic,
            "only",
            "one item",
            0.5,
        )];
        let plan = default_plan(5);
        let result = MemoryDiversifyPipeline::diversify(candidates, &plan, default_options());
        assert_eq!(result.len(), 1);
    }

    fn count_high_similarity_pairs(candidates: &[ScoredMemory], threshold: f32) -> usize {
        let mut count = 0;
        for (i, a) in candidates.iter().enumerate() {
            for b in candidates.iter().skip(i + 1) {
                if item_similarity(a, b) >= threshold {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn effective_min_slots_includes_required_preference() {
        let plan = RecallPlan {
            required_kinds: vec![
                MemoryKind::Semantic,
                MemoryKind::Episodic,
                MemoryKind::Preference,
            ],
            ..default_plan(8)
        };
        let options = MemoryDiversifyOptions {
            min_slots_semantic: 0,
            min_slots_episodic: 0,
            min_slots_user_profile: 0,
            min_slots_commitment: 0,
            ..default_options()
        };

        let mins = effective_min_slots(&plan, options, 8);
        assert!(
            mins.iter()
                .any(|(kind, count)| *kind == MemoryKind::Preference && *count >= 1),
            "Preference from required_kinds should be allocated"
        );
    }

    #[test]
    fn preference_required_kind_preserved() {
        let mut episodic: Vec<ScoredMemory> = (1..=8)
            .map(|i| {
                sample_memory(
                    i,
                    MemoryKind::Episodic,
                    &format!("event {i}"),
                    &format!("Unrelated activity on day {i}"),
                    (i as f32).mul_add(-0.01, 0.9),
                )
            })
            .collect();
        episodic.push(sample_memory(
            99,
            MemoryKind::Preference,
            "favorite drink",
            "The user loves matcha latte",
            0.12,
        ));

        let plan = RecallPlan {
            required_kinds: vec![
                MemoryKind::Semantic,
                MemoryKind::Episodic,
                MemoryKind::Preference,
            ],
            ..default_plan(5)
        };
        let options = MemoryDiversifyOptions {
            min_slots_semantic: 0,
            min_slots_episodic: 0,
            min_slots_user_profile: 0,
            min_slots_commitment: 0,
            ..default_options()
        };
        let result = MemoryDiversifyPipeline::diversify(episodic, &plan, options);

        assert!(
            result.iter().any(|m| m.item.kind == MemoryKind::Preference),
            "preference should be preserved when listed in required_kinds"
        );
    }

    #[test]
    fn tight_budget_allocates_by_priority() {
        let plan = default_plan(3);
        let options = default_options();

        let mins = effective_min_slots(&plan, options, 3);
        let slots_for = |kind: MemoryKind| -> usize {
            mins.iter().find(|(k, _)| *k == kind).map_or(0, |(_, n)| *n)
        };

        assert_eq!(slots_for(MemoryKind::Commitment), 1);
        assert_eq!(slots_for(MemoryKind::UserProfile), 1);
        assert_eq!(slots_for(MemoryKind::Semantic), 1);
        assert_eq!(slots_for(MemoryKind::Episodic), 0);
    }

    #[test]
    fn mmr_reduces_near_duplicate_count() {
        let candidates = vec![
            sample_memory(
                1,
                MemoryKind::Episodic,
                "pizza a",
                "pepperoni pizza friday tradition family",
                0.95,
            ),
            sample_memory(
                2,
                MemoryKind::Episodic,
                "pizza b",
                "pepperoni pizza friday tradition family night",
                0.94,
            ),
            sample_memory(
                3,
                MemoryKind::Episodic,
                "pizza c",
                "pepperoni pizza friday tradition family dinner",
                0.93,
            ),
            sample_memory(
                4,
                MemoryKind::Episodic,
                "pizza d",
                "pepperoni pizza friday tradition family fun",
                0.92,
            ),
            sample_memory(
                5,
                MemoryKind::Episodic,
                "pizza e",
                "pepperoni pizza friday tradition family time",
                0.91,
            ),
            sample_memory(
                6,
                MemoryKind::Semantic,
                "programming",
                "Rust ownership and borrowing rules",
                0.4,
            ),
        ];

        let plan = default_plan(5);
        let options = MemoryDiversifyOptions {
            min_slots_semantic: 0,
            min_slots_episodic: 0,
            min_slots_user_profile: 0,
            min_slots_commitment: 0,
            ..default_options()
        };

        let input_pairs = count_high_similarity_pairs(&candidates, 0.5);
        let result = MemoryDiversifyPipeline::diversify(candidates, &plan, options);
        let output_pairs = count_high_similarity_pairs(&result, 0.5);

        assert!(
            output_pairs < input_pairs,
            "MMR should reduce near-duplicate pairs (input={input_pairs}, output={output_pairs})"
        );
    }
}
