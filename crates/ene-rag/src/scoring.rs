//! Hybrid memory search scoring (pure functions, #302, #346).
//!
//! Moved from `ene-store::search` — combines vector similarity, lexical
//! overlap, recency, salience, affect, relationship, and access signals into
//! an explainable score breakdown. No DB I/O lives here; `ene-store` gathers
//! candidates and this layer scores them.
//!
//! ## Score structure (#346)
//!
//! The combination is *relevance-driven*, not additive:
//!
//! ```text
//! total = (relevance × quality_factor + commitment_boost) × penalty_multiplier
//! ```
//!
//! - `relevance` blends vector similarity and lexical overlap — it decides
//!   whether a memory is a candidate at all.
//! - `quality_factor` (`>= 1.0`) rescales relevant memories by their intrinsic
//!   quality; it can reorder candidates but never lift an irrelevant one.
//! - `commitment_boost` is added *inside* the penalty multiplier, so an active
//!   promise still surfaces with zero query relevance but a disputed/faded/
//!   expired commitment is discounted along with the rest of its score.
//! - `penalty_multiplier` (`(0, 1]`) applies contradiction/stale penalties as a
//!   scale-invariant fraction rather than an absolute subtraction.
//!
//! This replaces the previous weighted sum, in which the quality components
//! collectively outweighed relevance roughly 2:1 and could push an unrelated
//! memory above a strongly relevant one.
//!
//! ## Access boost is time-decayed (#345)
//!
//! The access component of [`quality_factor`] still rewards frequently-used
//! memories, but the reward now fades with the age of the most recent access
//! (see [`access_boost_score`]). Combined with the recall bump being gated on
//! actual prompt inclusion, this stops a memory's accumulated access count from
//! permanently inflating its score — old accesses stop counting.

use chrono::{DateTime, Utc};
use ene_core::{
    AffectAnnotation, GatheredCandidate, HybridSearchWeights, MemoryCandidateSource,
    MemoryScoreBreakdown, MemoryStatus, Query,
};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

use crate::decay::recency_score;

/// Half-life in days for the access-boost recency decay (#345).
///
/// Access history fades on this timescale so that old accesses stop boosting a
/// memory's recall score. Chosen shorter than the typical content-forgetting
/// half-life so the ranking reward for being "well-used" recedes well before
/// the memory itself would fade.
pub const ACCESS_BOOST_HALF_LIFE_DAYS: f64 = 14.0;

/// Whether a character belongs to a CJK script that is written without spaces
/// between words (Han ideographs, Hiragana, Katakana, Hangul), plus the CJK
/// iteration/ideographic marks (e.g. `々`, `〇`, `〆`).
///
/// These are the ranges relevant to Japanese (plus Hangul for Korean); half-width
/// and compatibility forms are folded into these ranges by NFKC normalization in
/// [`tokenize`] before this is consulted. The iteration marks are included because
/// `char::is_alphanumeric()` returns `true` for them (category `Lm`), so without
/// this they would be misrouted through the non-CJK word path and split common
/// words like `人々` / `時々` into lone unigrams.
fn is_cjk(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0x3005..=0x3007   // Ideographic iteration (々), closing (〆), zero (〇) marks
        | 0x3031..=0x3035 // Vertical kana repeat marks (〱–〵)
        | 0x303B..=0x303C // Vertical ideographic iteration (〻) and masu (〼) marks
        | 0x3040..=0x309F // Hiragana
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

/// Diminishing-returns access boost that decays with time since last access (#345).
///
/// The count contribution is the same saturating curve as before
/// (`1 − exp(−count·0.25)`, ~0.92 at ten accesses), but it is multiplied by a
/// half-life decay on the *age* of the most recent access. Old accesses stop
/// mattering: a memory recalled ten times a year ago no longer carries a near
/// full boost, so access history cannot lock a memory into the top of the
/// ranking forever. A memory that has never been accessed (`last_accessed_at`
/// `None`) scores `0.0` regardless of its stored count.
pub fn access_boost_score(
    access_count: i64,
    last_accessed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    access_half_life_days: f64,
) -> f32 {
    if access_count <= 0 {
        return 0.0;
    }
    let Some(last_accessed) = last_accessed_at else {
        return 0.0;
    };
    let count_factor = (1.0 - (-(access_count as f32) * 0.25).exp()).clamp(0.0, 1.0);
    let recency = crate::decay::half_life_decay(
        crate::decay::age_in_days(now, last_accessed),
        access_half_life_days,
    );
    (count_factor * recency).clamp(0.0, 1.0)
}

/// Penalty for disputed memories.
pub fn contradiction_penalty(status: MemoryStatus) -> f32 {
    if status == MemoryStatus::Disputed {
        0.30
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
        penalty += 0.20;
    }
    if let Some(until) = valid_until
        && until < now
    {
        penalty += 0.40;
    }
    penalty
}

/// Combined query-relevance signal in `[0, 1]` (#346).
///
/// Blends vector cosine similarity and lexical overlap with their relevance
/// weights. This is the score *base*: it decides whether a memory is a
/// candidate at all. A memory that is neither vector-similar nor lexically
/// overlapping has relevance zero and cannot be lifted by quality alone.
pub fn relevance_score(vector_similarity: f32, lexical: f32, weights: HybridSearchWeights) -> f32 {
    let vector_weight = weights.vector.max(0.0);
    let lexical_weight = weights.lexical.max(0.0);
    let total_weight = vector_weight + lexical_weight;
    if total_weight <= 0.0 {
        return 0.0;
    }
    let vector = if vector_similarity.is_nan() {
        0.0
    } else {
        vector_similarity.clamp(0.0, 1.0)
    };
    let lexical = if lexical.is_nan() {
        0.0
    } else {
        lexical.clamp(0.0, 1.0)
    };
    (vector * vector_weight + lexical * lexical_weight) / total_weight
}

/// Multiplicative quality factor `>= 1.0` (#346).
///
/// Combines the memory's intrinsic quality signals — recency, salience,
/// confidence, affect match, relationship, and access history — into
/// `1 + Σ(component × weight)`. It rescales the relevance base rather than
/// competing with it, so a high-quality memory can outrank a similarly
/// relevant one but can never overtake a much more relevant memory (the
/// additive-structure bug in #346).
pub fn quality_factor(
    recency: f32,
    salience: f32,
    confidence: f32,
    emotional: f32,
    relationship: f32,
    access: f32,
    weights: HybridSearchWeights,
) -> f32 {
    let component = |value: f32, weight: f32| -> f32 {
        if value.is_nan() || weight <= 0.0 {
            0.0
        } else {
            value.clamp(0.0, 1.0) * weight
        }
    };
    1.0 + component(recency, weights.recency)
        + component(salience, weights.salience)
        + component(confidence, weights.confidence)
        + component(emotional, weights.emotional_match)
        + component(relationship, weights.relationship)
        + component(access, weights.access_boost)
}

/// Scale-invariant penalty multiplier in `(0, 1]` (#346).
///
/// Converts the absolute contradiction and stale penalties into a single
/// multiplicative retention factor `1 / (1 + Σpenalty)`, so a penalty removes
/// the same *fraction* of a score regardless of its magnitude. The old
/// additive subtraction was asymmetric — near-fatal for weak candidates and
/// negligible for strong ones.
pub fn penalty_multiplier(contradiction: f32, stale: f32) -> f32 {
    let contradiction = if contradiction.is_nan() {
        0.0
    } else {
        contradiction.max(0.0)
    };
    let stale = if stale.is_nan() { 0.0 } else { stale.max(0.0) };
    1.0 / (1.0 + contradiction + stale)
}

/// Compute the hybrid score breakdown for a gathered candidate (#346).
///
/// Total is `(relevance × quality_factor + commitment_boost) × penalty_multiplier`,
/// clamped to be non-negative. Relevance (vector similarity plus lexical overlap)
/// is the multiplicative base, quality only rescales it, penalties are a
/// scale-invariant multiplier, and the commitment boost — added before the
/// penalty multiplier — lets an active promise surface even with zero query
/// relevance while still being discounted for disputed/faded/expired status.
pub fn score_candidate(options: &Query<'_>, candidate: &GatheredCandidate) -> MemoryScoreBreakdown {
    let item = &candidate.item;
    let lexical = lexical_overlap_score(options.query_text, &item.title, &item.content);
    let recency = recency_score(options.now, item, options.decay_half_life_days);
    let salience = item.salience.get();
    let confidence = item.confidence.get();
    let emotional = emotional_match_score(options.query_affect, item.affect);
    let relationship = relationship_score(item.relationship_impact);
    let access = access_boost_score(
        item.access_count,
        item.last_accessed_at,
        options.now,
        ACCESS_BOOST_HALF_LIFE_DAYS,
    );
    let contradiction = contradiction_penalty(item.status);
    let stale = stale_penalty(item.status, options.now, item.valid_until);

    let w = options.weights;
    let relevance = relevance_score(candidate.vector_similarity, lexical, w);
    let quality = quality_factor(
        recency,
        salience,
        confidence,
        emotional,
        relationship,
        access,
        w,
    );
    let penalty = penalty_multiplier(contradiction, stale);

    let commitment_boost = if candidate
        .sources
        .contains(&MemoryCandidateSource::Commitment)
    {
        options.commitment_boost
    } else {
        0.0
    };

    let total = (relevance.mul_add(quality, commitment_boost)) * penalty;
    let total = if total.is_nan() || total < 0.0 {
        0.0
    } else {
        total
    };

    MemoryScoreBreakdown {
        vector_similarity: candidate.vector_similarity,
        lexical_score: lexical,
        recency_score: recency,
        salience,
        confidence,
        emotional_match: emotional,
        relationship,
        access_boost: access,
        relevance,
        quality_factor: quality,
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
///
/// Recent-fallback candidates are exempt from the `min_score` gate: they are
/// deliberately injected conversational context (bounded by
/// `recent_fallback_limit`), not relevance matches, so their hybrid total is
/// `0.0` by construction. Filtering them out would defeat the fallback.
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
        .filter(|scored| {
            scored.breakdown.total >= options.min_score
                || scored.sources.contains(&MemoryCandidateSource::Recent)
        })
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
    fn tokenize_keeps_iteration_mark_in_bigrams() {
        // The ideographic iteration mark `々` (category Lm) is alphanumeric, so it
        // must be classified as CJK or it would split words like 人々/時々 into
        // lone unigrams (Bugbot regression guard).
        let tokens = tokenize("人々時々色々");
        assert!(tokens.contains("人々"));
        assert!(tokens.contains("時々"));
        assert!(tokens.contains("色々"));
        assert!(
            !tokens.contains("々"),
            "々 must not be emitted as a standalone token, got {tokens:?}"
        );
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

    #[test]
    fn recent_fallback_candidates_survive_min_score_gate() {
        // Recent-fallback rows are deliberately injected context with total 0.0
        // under the multiplicative formula; they must not be dropped by
        // min_score, or the fallback would be a no-op.
        let now = Utc::now();
        let mut options = sample_query(now);
        options.min_score = 0.5;

        let recent = GatheredCandidate {
            item: item_with_quality(1.0, 1.0, 100),
            vector_similarity: 0.0,
            sources: vec![MemoryCandidateSource::Recent],
        };
        let vector = GatheredCandidate {
            item: item_with_quality(0.1, 0.1, 0),
            vector_similarity: 0.4,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let ranked = score_and_rank(&options, vec![recent, vector]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].sources, vec![MemoryCandidateSource::Recent]);
    }

    fn item_with_quality(salience: f32, confidence: f32, access_count: i64) -> MemoryItem {
        let mut item = sample_item(MemoryStatus::Active);
        // Neutral text with no overlap against the "pizza" query, so relevance
        // is driven purely by the vector similarity each test supplies.
        item.title = "weather report".into();
        item.content = "sunny skies expected tomorrow".into();
        item.salience = MemorySalience::new(salience);
        item.confidence = MemoryConfidence::new(confidence);
        item.access_count = access_count;
        item
    }

    #[test]
    fn quality_cannot_override_relevance() {
        // Regression for #346: a barely-relevant memory with maximal quality
        // must not outrank a strongly-relevant memory with low quality.
        let now = Utc::now();
        let options = sample_query(now);

        let relevant = GatheredCandidate {
            item: item_with_quality(0.1, 0.1, 0),
            vector_similarity: 0.95,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let irrelevant = GatheredCandidate {
            item: item_with_quality(1.0, 1.0, 50),
            vector_similarity: 0.0,
            sources: vec![MemoryCandidateSource::Recent],
        };

        let ranked = score_and_rank(&options, vec![irrelevant, relevant]);
        assert_eq!(ranked.len(), 2);
        assert!(
            ranked[0].breakdown.vector_similarity > 0.9,
            "the strongly relevant memory must rank first, got {:?}",
            ranked[0].breakdown
        );
    }

    #[test]
    fn zero_relevance_scores_zero_without_commitment() {
        let now = Utc::now();
        let options = sample_query(now);
        let candidate = GatheredCandidate {
            item: item_with_quality(1.0, 1.0, 100),
            vector_similarity: 0.0,
            sources: vec![MemoryCandidateSource::Recent],
        };
        let breakdown = score_candidate(&options, &candidate);
        assert!(breakdown.relevance < f32::EPSILON);
        assert!(breakdown.total < f32::EPSILON);
    }

    #[test]
    fn commitment_surfaces_despite_zero_relevance() {
        let now = Utc::now();
        let options = sample_query(now);
        let candidate = GatheredCandidate {
            item: item_with_quality(0.5, 0.5, 0),
            vector_similarity: 0.0,
            sources: vec![MemoryCandidateSource::Commitment],
        };
        let breakdown = score_candidate(&options, &candidate);
        assert!(breakdown.commitment_boost > 0.0);
        assert!(breakdown.total >= breakdown.commitment_boost);
    }

    #[test]
    fn commitment_boost_is_discounted_by_penalties() {
        // The boost is applied *inside* the penalty multiplier, so a disputed
        // or faded commitment no longer surfaces at full strength.
        let now = Utc::now();
        let options = sample_query(now);

        let mut faded_item = item_with_quality(0.5, 0.5, 0);
        faded_item.status = MemoryStatus::Faded;
        let clean = score_candidate(
            &options,
            &GatheredCandidate {
                item: item_with_quality(0.5, 0.5, 0),
                vector_similarity: 0.0,
                sources: vec![MemoryCandidateSource::Commitment],
            },
        );
        let faded = score_candidate(
            &options,
            &GatheredCandidate {
                item: faded_item,
                vector_similarity: 0.0,
                sources: vec![MemoryCandidateSource::Commitment],
            },
        );
        assert!(clean.total >= clean.commitment_boost);
        assert!(faded.total > 0.0);
        assert!(
            faded.total < clean.total,
            "a faded commitment must be discounted below its clean score"
        );
    }

    #[test]
    fn penalty_is_scale_invariant() {
        // Regression for #346: penalties remove a constant *fraction* of the
        // score, so a disputed strong candidate keeps its lead over a disputed
        // weak one (the old absolute subtraction hit weak candidates hardest).
        let now = Utc::now();
        let options = sample_query(now);

        let strong = GatheredCandidate {
            item: item_with_quality(0.9, 0.9, 10),
            vector_similarity: 0.9,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let weak = GatheredCandidate {
            item: item_with_quality(0.2, 0.2, 0),
            vector_similarity: 0.4,
            sources: vec![MemoryCandidateSource::Vector],
        };

        let strong_clean = score_candidate(&options, &strong);
        let weak_clean = score_candidate(&options, &weak);

        let mut disputed_strong_item = strong.item.clone();
        disputed_strong_item.status = MemoryStatus::Disputed;
        let strong_disputed = score_candidate(
            &options,
            &GatheredCandidate {
                item: disputed_strong_item,
                vector_similarity: strong.vector_similarity,
                sources: strong.sources.clone(),
            },
        );
        let mut disputed_weak_item = weak.item.clone();
        disputed_weak_item.status = MemoryStatus::Disputed;
        let weak_disputed = score_candidate(
            &options,
            &GatheredCandidate {
                item: disputed_weak_item,
                vector_similarity: weak.vector_similarity,
                sources: weak.sources.clone(),
            },
        );

        // Same multiplicative retention factor regardless of magnitude.
        let strong_retained = strong_disputed.total / strong_clean.total;
        let weak_retained = weak_disputed.total / weak_clean.total;
        assert!((strong_retained - weak_retained).abs() < 1e-4);
        // The strong candidate still beats the weak one once both are disputed.
        assert!(strong_disputed.total > weak_disputed.total);
    }

    #[test]
    fn relevance_and_quality_breakdown_are_exposed() {
        let now = Utc::now();
        let options = sample_query(now);
        let candidate = GatheredCandidate {
            item: item_with_quality(0.7, 0.8, 3),
            vector_similarity: 0.6,
            sources: vec![MemoryCandidateSource::Vector],
        };
        let breakdown = score_candidate(&options, &candidate);
        assert!(breakdown.relevance > 0.0);
        assert!(breakdown.quality_factor >= 1.0);
        // total = relevance * quality * penalty (no commitment here).
        let expected = breakdown.relevance * breakdown.quality_factor;
        assert!((breakdown.total - expected).abs() < 1e-5);
    }

    #[test]
    fn access_boost_is_zero_without_accesses() {
        let now = Utc::now();
        assert!(access_boost_score(0, Some(now), now, 14.0) < f32::EPSILON);
        // A stored count with no recorded access time scores nothing: the
        // reward is gated on a real, datable access.
        assert!(access_boost_score(10, None, now, 14.0) < f32::EPSILON);
    }

    #[test]
    fn access_boost_fades_with_age_of_last_access() {
        // #345 regression: the same access count must not lock in a permanent
        // boost — an access from long ago fades toward zero.
        let now = Utc::now();
        let recent = now - chrono::Duration::days(1);
        let stale = now - chrono::Duration::days(365);
        let fresh_boost = access_boost_score(10, Some(recent), now, 14.0);
        let stale_boost = access_boost_score(10, Some(stale), now, 14.0);
        assert!(fresh_boost > 0.5, "recent accesses should still boost");
        assert!(
            stale_boost < 0.01,
            "year-old accesses must have decayed away, got {stale_boost}"
        );
        assert!(stale_boost < fresh_boost);
    }

    #[test]
    fn access_boost_is_half_at_one_half_life() {
        let now = Utc::now();
        let one_half_life_ago = now - chrono::Duration::days(14);
        // Saturating count factor ≈ 0.918 at count=10; half-life decay halves it.
        let full = access_boost_score(10, Some(now), now, 14.0);
        let half = access_boost_score(10, Some(one_half_life_ago), now, 14.0);
        assert!((half - full * 0.5).abs() < 1e-3);
    }
}
