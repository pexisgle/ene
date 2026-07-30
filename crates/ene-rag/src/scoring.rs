//! Hybrid memory search scoring (pure functions, #302).
//!
//! Moved from `ene-store::search` — combines vector similarity, lexical
//! overlap, recency, salience, affect, relationship, and access signals into
//! an explainable score breakdown. No DB I/O lives here; `ene-store` gathers
//! candidates and this layer scores them.

use chrono::{DateTime, Utc};
use ene_core::{
    AffectAnnotation, GatheredCandidate, MemoryCandidateSource, MemoryScoreBreakdown, MemoryStatus,
    Query,
};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

use crate::decay::recency_score;

/// Whether a character belongs to a CJK script that is written without spaces
/// between words (Han ideographs, Hiragana, Katakana, Hangul).
///
/// These are the ranges relevant to Japanese (plus Hangul for Korean); half-width
/// and compatibility forms are folded into these ranges by NFKC normalization in
/// [`tokenize`] before this is consulted.
fn is_cjk(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0x3040..=0x309F   // Hiragana
        | 0x30A0..=0x30FF // Katakana
        | 0x31F0..=0x31FF // Katakana phonetic extensions
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0x2_0000..=0x2_A6DF // CJK Extension B
        | 0x2_A700..=0x2_EBEF // CJK Extensions C–F
    )
}

/// Flush a pending non-CJK alphanumeric word run as a single token.
fn flush_word(run: &mut String, tokens: &mut HashSet<String>) {
    if !run.is_empty() {
        tokens.insert(std::mem::take(run));
    }
}

/// Flush a pending CJK run as overlapping bigrams (or a lone unigram).
fn flush_cjk(run: &mut Vec<char>, tokens: &mut HashSet<String>) {
    match run.len() {
        0 => {}
        // A single CJK character cannot form a bigram; keep it as a unigram so
        // lone-character terms still participate in matching.
        1 => {
            tokens.insert(run[0].to_string());
        }
        _ => {
            for pair in run.windows(2) {
                let mut bigram = String::with_capacity(pair[0].len_utf8() + pair[1].len_utf8());
                bigram.push(pair[0]);
                bigram.push(pair[1]);
                tokens.insert(bigram);
            }
        }
    }
    run.clear();
}

/// Tokenize text for lexical overlap.
///
/// Text is NFKC-normalized (folding full-width alphanumerics and half-width
/// kana into their canonical forms) and lowercased, then split into terms:
///
/// - Runs of non-CJK alphanumerics (Latin, Cyrillic, …) become whole-word
///   tokens, as before — `"hello world"` → `{hello, world}`.
/// - Runs of CJK characters (which are written without inter-word spaces) are
///   decomposed into overlapping bigrams, the standard dictionary-free
///   approach for Japanese/Chinese — `"今日は良い天気"` →
///   `{今日, 日は, は良, 良い, い天, 天気}`. A lone CJK character becomes a
///   unigram.
///
/// This restores the lexical (term-overlap) component of hybrid search for
/// Japanese, which the previous whitespace/ASCII splitter reduced to a single
/// opaque token per sentence (#303). Bigram tokenization is deliberately
/// dictionary-free (no MeCab/lindera) so it adds no native build dependency or
/// dictionary download to CI; the trade-off is that single-character queries
/// only match documents where that character appears alone.
pub fn tokenize(text: &str) -> HashSet<String> {
    let normalized: String = text.nfkc().collect::<String>().to_lowercase();
    let mut tokens = HashSet::new();
    let mut word = String::new();
    let mut cjk_run: Vec<char> = Vec::new();

    for ch in normalized.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut tokens);
            cjk_run.push(ch);
        } else {
            flush_cjk(&mut cjk_run, &mut tokens);
            if ch.is_alphanumeric() {
                word.push(ch);
            } else {
                flush_word(&mut word, &mut tokens);
            }
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);
    tokens
}

/// Jaccard similarity between two memory documents (title + content tokens).
///
/// Used for duplicate clustering and MMR pairwise diversity (#78).
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
pub fn lexical_overlap_score(query: &str, title: &str, content: &str) -> f32 {
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

/// Match between query affect and memory affect in `[0.0, 1.0]`.
pub fn emotional_match_score(
    query_affect: Option<AffectAnnotation>,
    item_affect: AffectAnnotation,
) -> f32 {
    let Some(query) = query_affect else {
        return 0.0;
    };
    let dv = query.valence - item_affect.valence;
    let da = query.arousal - item_affect.arousal;
    let dist = dv.hypot(da);
    if dist.is_nan() {
        0.0
    } else {
        // Max distance in unit square diagonals ~2.83; map to similarity.
        (1.0 - (dist / 2.83)).clamp(0.0, 1.0)
    }
}

/// Normalize relationship impact `[-1, 1]` to `[0, 1]`.
pub fn relationship_score(impact: f32) -> f32 {
    if impact.is_nan() {
        0.5
    } else {
        f32::midpoint(impact, 1.0).clamp(0.0, 1.0)
    }
}

/// Diminishing returns boost from prior accesses.
pub fn access_boost_score(access_count: i64) -> f32 {
    if access_count <= 0 {
        return 0.0;
    }
    (1.0 - (-(access_count as f32) * 0.25).exp()).clamp(0.0, 1.0)
}

/// Penalty for disputed memories.
pub fn contradiction_penalty(status: MemoryStatus) -> f32 {
    if status == MemoryStatus::Disputed {
        0.15
    } else {
        0.0
    }
}

/// Penalty for faded or expired memories.
pub fn stale_penalty(
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

/// Compute weighted hybrid score breakdown for a gathered candidate.
pub fn score_candidate(options: &Query<'_>, candidate: &GatheredCandidate) -> MemoryScoreBreakdown {
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
    let total = if total.is_nan() { 0.0 } else { total };

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

/// Score, filter, sort, and truncate gathered candidates into ranked results.
///
/// Reproduces the post-gather pipeline that `MemoryStore::search` used to run
/// inline (#302): score every candidate, drop rows below `query.min_score`,
/// order by total (then vector similarity, then recency), and cap at
/// `query.limit`. Time-range filtering is applied here too so callers get the
/// same result set the store previously returned.
pub fn score_and_rank(
    options: &Query<'_>,
    candidates: Vec<GatheredCandidate>,
) -> Vec<ene_core::ScoredMemory> {
    let mut scored: Vec<ene_core::ScoredMemory> = candidates
        .into_iter()
        .filter(|candidate| {
            within_time_range(options.time_range.as_ref(), candidate.item.created_at)
        })
        .map(|candidate| {
            let breakdown = score_candidate(options, &candidate);
            ene_core::ScoredMemory {
                item: candidate.item,
                breakdown,
                sources: candidate.sources,
            }
        })
        .filter(|scored| scored.breakdown.total >= options.min_score)
        .collect();

    scored.sort_by(|a, b| {
        b.breakdown
            .total
            .total_cmp(&a.breakdown.total)
            .then_with(|| {
                b.breakdown
                    .vector_similarity
                    .total_cmp(&a.breakdown.vector_similarity)
            })
            .then_with(|| b.item.updated_at.cmp(&a.item.updated_at))
    });

    if scored.len() > options.limit {
        scored.truncate(options.limit);
    }
    scored
}

/// Whether a memory's `created_at` falls within the optional time range.
///
/// Returns `true` when `range` is `None` (no filter) or when the timestamp is
/// within the inclusive `[start, end]` bounds. Missing bounds are unbounded.
pub fn within_time_range(range: Option<&ene_core::TimeRange>, created_at: DateTime<Utc>) -> bool {
    let Some(range) = range else {
        return true;
    };
    if let Some(start) = range.start
        && created_at < start
    {
        return false;
    }
    if let Some(end) = range.end
        && created_at > end
    {
        return false;
    }
    true
}

/// Whether a memory status is eligible for normal hybrid recall.
pub const fn is_recallable_status(status: MemoryStatus) -> bool {
    matches!(
        status,
        MemoryStatus::Active | MemoryStatus::Faded | MemoryStatus::Disputed
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use ene_core::{
        HybridSearchWeights, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemorySource, TimeRange,
    };

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

    fn sample_query(now: DateTime<Utc>) -> Query<'static> {
        Query {
            query_text: "pizza",
            embedding: None,
            character_id: "ene",
            user_id: None,
            model_name: "test",
            limit: 5,
            similarity_threshold: 0.0,
            candidate_pool_size: 50,
            query_affect: None,
            weights: HybridSearchWeights::default(),
            time_range: None,
            decay_half_life_days: 30.0,
            now,
            min_score: 0.0,
            commitment_boost: 0.25,
            recent_fallback_limit: 5,
        }
    }

    #[test]
    fn lexical_overlap_matches_query_tokens() {
        let score = lexical_overlap_score("pizza favorite", "favorite food", "likes pizza");
        assert!(score > 0.0);
        assert!(lexical_overlap_score("", "a", "b") < f32::EPSILON);
    }

    #[test]
    fn tokenize_splits_japanese_into_bigrams() {
        // The whole point of #303: a Japanese sentence must not collapse into a
        // single opaque token.
        let tokens = tokenize("今日は良い天気ですね");
        assert!(
            tokens.len() > 1,
            "Japanese text must produce multiple tokens, got {tokens:?}"
        );
        assert!(tokens.contains("今日"));
        assert!(tokens.contains("天気"));
    }

    #[test]
    fn tokenize_keeps_english_words_whole() {
        // Regression guard: the CJK path must not disturb ASCII word tokens.
        let tokens = tokenize("hello world foo");
        assert_eq!(tokens.len(), 3);
        assert!(tokens.contains("hello"));
        assert!(tokens.contains("world"));
        assert!(tokens.contains("foo"));
    }

    #[test]
    fn tokenize_handles_katakana() {
        let tokens = tokenize("エネは可愛い、とても可愛い");
        assert!(
            tokens.len() > 2,
            "katakana/hiragana runs must be bigram-split, got {tokens:?}"
        );
        assert!(tokens.contains("エネ"));
        assert!(tokens.contains("可愛"));
    }

    #[test]
    fn tokenize_lone_cjk_char_is_unigram() {
        let tokens = tokenize("猫");
        assert!(tokens.contains("猫"));
        assert_eq!(tokens.len(), 1);
    }

    #[test]
    fn tokenize_folds_fullwidth_alphanumerics() {
        // NFKC folds full-width forms into ASCII so they match ASCII queries.
        let tokens = tokenize("Ｈｅｌｌｏ１２３");
        assert!(tokens.contains("hello123"));
    }

    #[test]
    fn tokenize_mixed_japanese_and_english() {
        let tokens = tokenize("Rustで書く hello");
        assert!(tokens.contains("rust"));
        assert!(tokens.contains("hello"));
        // "で書く" is a 3-char CJK run → two bigrams.
        assert!(tokens.contains("で書"));
        assert!(tokens.contains("書く"));
    }

    #[test]
    fn lexical_overlap_scores_japanese() {
        // Before #303 this returned 0.0 for Japanese queries.
        let score = lexical_overlap_score("良い天気", "天気予報", "今日は良い天気ですね");
        assert!(
            score > 0.0,
            "Japanese query must contribute lexical overlap"
        );
    }

    #[test]
    fn document_lexical_similarity_japanese_overlap() {
        let sim = document_lexical_similarity(
            "天気",
            "今日は良い天気ですね",
            "天気予報",
            "明日の天気は晴れです",
        );
        assert!(
            sim > 0.0,
            "Japanese documents sharing terms must have positive similarity"
        );
        assert!(sim < 1.0);
    }

    #[test]
    fn document_lexical_similarity_japanese_identical() {
        let sim = document_lexical_similarity(
            "好きな食べ物",
            "ユーザーはピザが好き",
            "好きな食べ物",
            "ユーザーはピザが好き",
        );
        assert!((sim - 1.0).abs() < f32::EPSILON);
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
        let options = sample_query(now);
        let candidate = GatheredCandidate {
            item: sample_item(MemoryStatus::Faded),
            vector_similarity: 0.2,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let breakdown = score_candidate(&options, &candidate);
        assert!(breakdown.total >= 0.0);
        assert!(breakdown.stale_penalty > 0.0);
    }

    #[test]
    fn nan_inputs_are_handled_gracefully() {
        assert!((relationship_score(f32::NAN) - 0.5).abs() < f32::EPSILON);

        let now = Utc::now();
        let item = sample_item(MemoryStatus::Active);
        assert!((recency_score(now, &item, f64::NAN) - 1.0).abs() < f32::EPSILON);

        let q_affect = Some(AffectAnnotation {
            valence: f32::NAN,
            arousal: 0.0,
        });
        let item_affect = AffectAnnotation {
            valence: 0.0,
            arousal: 0.0,
        };
        assert!((emotional_match_score(q_affect, item_affect) - 0.0).abs() < f32::EPSILON);

        let options = sample_query(now);
        let candidate = GatheredCandidate {
            item: sample_item(MemoryStatus::Active),
            vector_similarity: f32::NAN,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let breakdown = score_candidate(&options, &candidate);
        assert!(!breakdown.total.is_nan());
        assert!(breakdown.total >= 0.0);
    }

    #[test]
    fn within_time_range_filters_by_created_at() {
        let created = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
        let before = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 7, 20, 0, 0, 0).unwrap();

        // No range => always included.
        assert!(within_time_range(None, created));

        // Inclusive bounds.
        let range = TimeRange {
            start: Some(before),
            end: Some(after),
        };
        assert!(within_time_range(Some(&range), created));
        assert!(within_time_range(Some(&range), before));
        assert!(within_time_range(Some(&range), after));

        // Out of range on both sides.
        let too_early = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let too_late = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        assert!(!within_time_range(Some(&range), too_early));
        assert!(!within_time_range(Some(&range), too_late));

        // Open-ended bounds.
        let start_only = TimeRange {
            start: Some(before),
            end: None,
        };
        assert!(within_time_range(Some(&start_only), too_late));
        assert!(!within_time_range(Some(&start_only), too_early));

        let end_only = TimeRange {
            start: None,
            end: Some(after),
        };
        assert!(within_time_range(Some(&end_only), too_early));
        assert!(!within_time_range(Some(&end_only), too_late));
    }

    #[test]
    fn score_and_rank_filters_sorts_and_truncates() {
        let now = Utc::now();
        let mut options = sample_query(now);
        options.limit = 1;
        options.min_score = 0.0;

        let strong = GatheredCandidate {
            item: sample_item(MemoryStatus::Active),
            vector_similarity: 0.9,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let weak = GatheredCandidate {
            item: sample_item(MemoryStatus::Faded),
            vector_similarity: 0.0,
            sources: vec![MemoryCandidateSource::Recent],
        };
        let ranked = score_and_rank(&options, vec![weak, strong]);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].breakdown.vector_similarity > 0.5);
    }
}
