//! Hybrid memory search scoring (pure functions).
//!
//! Combines vector similarity, lexical overlap, recency, salience, affect,
//! relationship, and access signals into an explainable score breakdown.

use crate::typed_memory::{
    AffectAnnotation, MemoryCandidateSource, MemoryItem, MemoryScoreBreakdown, MemoryStatus, Query,
};
use chrono::{DateTime, Utc};
use std::collections::HashSet;

/// Internal candidate gathered from one or more recall sources.
#[derive(Debug, Clone)]
pub(crate) struct GatheredCandidate {
    pub item: MemoryItem,
    pub vector_similarity: f32,
    pub sources: Vec<MemoryCandidateSource>,
}

/// Tokenize text for lexical overlap (lowercase alphanumeric tokens).
pub(crate) fn tokenize(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Jaccard similarity between two memory documents (title + content tokens).
///
/// Used for duplicate clustering and MMR pairwise diversity (#78).
#[must_use]
pub fn document_lexical_similarity(
    title_a: &str,
    content_a: &str,
    title_b: &str,
    content_b: &str,
) -> f32 {
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
    intersection as f32 / union as f32
}

/// Jaccard-like overlap between query tokens and document tokens.
pub(crate) fn lexical_overlap_score(query: &str, title: &str, content: &str) -> f32 {
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return 0.0;
    }
    let doc_tokens: HashSet<String> = tokenize(title)
        .into_iter()
        .chain(tokenize(content))
        .collect();
    if doc_tokens.is_empty() {
        return 0.0;
    }
    let overlap = query_tokens.intersection(&doc_tokens).count();
    overlap as f32 / query_tokens.len() as f32
}

/// Exponential recency score in `[0.0, 1.0]` using half-life decay in days.
pub(crate) fn recency_score(
    reference: DateTime<Utc>,
    item: &MemoryItem,
    half_life_days: f64,
) -> f32 {
    if half_life_days <= 0.0 {
        return 1.0;
    }
    let anchor = item
        .last_accessed_at
        .or(Some(item.updated_at))
        .unwrap_or(item.created_at);
    let age_secs = (reference - anchor).num_seconds().max(0) as f64;
    let age_days = age_secs / 86_400.0;
    let lambda = std::f64::consts::LN_2 / half_life_days;
    (-lambda * age_days).exp() as f32
}

/// Match between query affect and memory affect in `[0.0, 1.0]`.
pub(crate) fn emotional_match_score(
    query_affect: Option<AffectAnnotation>,
    item_affect: AffectAnnotation,
) -> f32 {
    let Some(query) = query_affect else {
        return 0.0;
    };
    let dv = query.valence - item_affect.valence;
    let da = query.arousal - item_affect.arousal;
    let dist = dv.hypot(da);
    // Max distance in unit square diagonals ~2.83; map to similarity.
    (1.0 - (dist / 2.83)).clamp(0.0, 1.0)
}

/// Normalize relationship impact `[-1, 1]` to `[0, 1]`.
pub(crate) const fn relationship_score(impact: f32) -> f32 {
    f32::midpoint(impact, 1.0).clamp(0.0, 1.0)
}

/// Diminishing returns boost from prior accesses.
pub(crate) fn access_boost_score(access_count: i64) -> f32 {
    if access_count <= 0 {
        return 0.0;
    }
    (1.0 - (-(access_count as f32) * 0.25).exp()).clamp(0.0, 1.0)
}

/// Penalty for disputed memories.
pub(crate) fn contradiction_penalty(status: MemoryStatus) -> f32 {
    if status == MemoryStatus::Disputed {
        0.15
    } else {
        0.0
    }
}

/// Penalty for faded or expired memories.
pub(crate) fn stale_penalty(
    status: MemoryStatus,
    now: DateTime<Utc>,
    valid_until: Option<DateTime<Utc>>,
) -> f32 {
    let mut penalty = 0.0;
    if status == MemoryStatus::Faded {
        penalty += 0.10;
    }
    if let Some(until) = valid_until
        && until < now
    {
        penalty += 0.20;
    }
    penalty
}

/// Whether a memory status is eligible for normal hybrid recall.
pub(crate) const fn is_recallable_status(status: MemoryStatus) -> bool {
    matches!(
        status,
        MemoryStatus::Active | MemoryStatus::Faded | MemoryStatus::Disputed
    )
}

/// Compute weighted hybrid score breakdown for a gathered candidate.
pub(crate) fn score_candidate(
    options: &Query<'_>,
    candidate: &GatheredCandidate,
) -> MemoryScoreBreakdown {
    let item = &candidate.item;
    let lexical = lexical_overlap_score(options.query_text, &item.title, &item.content);
    let recency = recency_score(options.now, item, options.decay_half_life_days);
    let salience = item.salience.get();
    let confidence = item.confidence.get();
    let emotional = emotional_match_score(options.query_affect, item.affect);
    let relationship = relationship_score(item.relationship_impact);
    let access = access_boost_score(item.access_count);
    let contradiction = contradiction_penalty(item.status);
    let stale = stale_penalty(item.status, options.now, item.valid_until);

    let w = &options.weights;
    let weighted = access.mul_add(
        w.access_boost,
        relationship.mul_add(
            w.relationship,
            emotional.mul_add(
                w.emotional_match,
                confidence.mul_add(
                    w.confidence,
                    salience.mul_add(
                        w.salience,
                        recency.mul_add(
                            w.recency,
                            lexical.mul_add(w.lexical, candidate.vector_similarity * w.vector),
                        ),
                    ),
                ),
            ),
        ),
    );

    let commitment_boost = if candidate
        .sources
        .contains(&MemoryCandidateSource::Commitment)
    {
        options.commitment_boost
    } else {
        0.0
    };

    let total = (weighted + commitment_boost - contradiction - stale).max(0.0);

    MemoryScoreBreakdown {
        vector_similarity: candidate.vector_similarity,
        lexical_score: lexical,
        recency_score: recency,
        salience,
        confidence,
        emotional_match: emotional,
        relationship,
        access_boost: access,
        contradiction_penalty: contradiction,
        stale_penalty: stale,
        commitment_boost,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typed_memory::{
        HybridSearchWeights, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
        MemorySource,
    };
    use chrono::TimeZone;

    fn sample_item(status: MemoryStatus) -> MemoryItem {
        MemoryItem {
            id: Some(1),
            scope: MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "favorite food".into(),
            content: "The user likes pizza".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::new(0.7),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.2,
            access_count: 2,
            last_accessed_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap(),
            valid_from: None,
            valid_until: None,
            status,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    #[test]
    fn lexical_overlap_matches_query_tokens() {
        let score = lexical_overlap_score("pizza favorite", "favorite food", "likes pizza");
        assert!(score > 0.0);
        assert!(lexical_overlap_score("", "a", "b") < f32::EPSILON);
    }

    #[test]
    fn document_lexical_similarity_identical_content() {
        let sim = document_lexical_similarity(
            "favorite food",
            "The user likes pizza",
            "favorite food",
            "The user likes pizza",
        );
        assert!((sim - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn document_lexical_similarity_unrelated_content() {
        let sim = document_lexical_similarity(
            "weather",
            "It was sunny today",
            "programming",
            "Rust ownership is important",
        );
        assert!(sim < 0.1);
    }

    #[test]
    fn document_lexical_similarity_partial_overlap() {
        let sim = document_lexical_similarity(
            "pizza night",
            "We ordered pepperoni pizza",
            "pizza tradition",
            "Family pizza night every Friday",
        );
        assert!(sim > 0.2);
        assert!(sim < 1.0);
    }

    #[test]
    fn recency_decays_with_age() {
        let now = Utc.with_ymd_and_hms(2026, 7, 3, 12, 0, 0).unwrap();
        let item = sample_item(MemoryStatus::Active);
        let fresh = recency_score(now, &item, 30.0);
        let mut old = item;
        old.updated_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let aged = recency_score(now, &old, 30.0);
        assert!(fresh > aged);
    }

    #[test]
    fn stale_penalty_for_faded_and_expired() {
        let now = Utc.with_ymd_and_hms(2026, 7, 3, 12, 0, 0).unwrap();
        assert!(stale_penalty(MemoryStatus::Faded, now, None) > 0.0);
        let expired = Some(Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap());
        assert!(stale_penalty(MemoryStatus::Active, now, expired) > 0.0);
    }

    #[test]
    fn score_breakdown_total_is_non_negative() {
        let now = Utc::now();
        let options = Query {
            query_text: "pizza",
            embedding: Some(&[0.1, 0.2, 0.3, 0.4]),
            character_id: "ene",
            user_id: None,
            model_name: "test",
            limit: 5,
            similarity_threshold: 0.0,
            candidate_pool_size: 50,
            query_affect: None,
            weights: HybridSearchWeights::default(),
            decay_half_life_days: 30.0,
            now,
            min_score: 0.0,
            commitment_boost: 0.25,
            recent_fallback_limit: 5,
        };
        let candidate = GatheredCandidate {
            item: sample_item(MemoryStatus::Faded),
            vector_similarity: 0.2,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let breakdown = score_candidate(&options, &candidate);
        assert!(breakdown.total >= 0.0);
        assert!(breakdown.stale_penalty > 0.0);
    }
}
