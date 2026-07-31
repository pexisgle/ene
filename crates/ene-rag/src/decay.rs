//! Decay scoring and lifecycle thresholds (#302).
//!
//! Consolidates the two independent half-life exponential-decay implementations
//! that previously lived in `ene-store` (`search::recency_score` for recall
//! ranking and `forgetting::decay_score` for lifecycle transitions) into a
//! single [`half_life_decay`] primitive. The two callers keep their distinct
//! anchors and post-processing; only the shared `exp(-λ·age)` kernel is unified.
//!
//! The anchors are deliberately different (#345): recall recency
//! ([`recency_score`]) measures from the last *access* (`last_accessed_at`),
//! while lifecycle forgetting ([`active_decay_anchor`]) measures from the last
//! *content update* (`updated_at`). Recall must never push the forgetting
//! anchor forward, or a frequently-recalled memory could never fade.
//!
//! Pure functions — no DB I/O. The state-machine validators
//! (`validate_transition` / `validate_user_restore`) remain in `ene-store`.

use chrono::{DateTime, Utc};
use ene_core::{AffectAnnotation, MemoryItem, MemoryStatus};

/// Score below which an [`MemoryStatus::Active`] memory transitions to [`MemoryStatus::Faded`].
pub const FADE_THRESHOLD: f32 = 0.40;

/// Score below which a [`MemoryStatus::Faded`] memory transitions to [`MemoryStatus::Archived`].
pub const ARCHIVE_THRESHOLD: f32 = 0.15;

/// Shared half-life exponential-decay kernel in `[0.0, 1.0]`.
///
/// Computes `exp(-(ln 2 / half_life_days) * age_days)`. A non-positive or
/// `NaN` half-life disables decay and returns `1.0`. Negative ages are
/// clamped to zero (a future anchor scores as brand-new).
///
/// This is the single implementation behind both recall [`recency_score`]
/// and lifecycle [`decay_score`]; they differ only in anchor selection and
/// the retention factors layered on top.
pub fn half_life_decay(age_days: f64, half_life_days: f64) -> f32 {
    if half_life_days <= 0.0 || half_life_days.is_nan() {
        return 1.0;
    }
    let age_days = age_days.max(0.0);
    let lambda = std::f64::consts::LN_2 / half_life_days;
    (-lambda * age_days).exp() as f32
}

/// Age in days of `anchor` relative to `reference` (negative ages clamped to 0).
pub(crate) fn age_in_days(reference: DateTime<Utc>, anchor: DateTime<Utc>) -> f64 {
    let age_secs = reference.signed_duration_since(anchor).num_seconds().max(0) as f64;
    age_secs / 86_400.0
}

/// Exponential recency score in `[0.0, 1.0]` using half-life decay in days.
///
/// Anchor: `last_accessed_at → updated_at`. Used for recall ranking among
/// recallable rows. This intentionally differs from the forgetting anchor
/// ([`active_decay_anchor`], which keys off `updated_at` only): recall rewards
/// memories that were recently *used*, while forgetting measures staleness of
/// the memory's *content* (#345).
pub fn recency_score(reference: DateTime<Utc>, item: &MemoryItem, half_life_days: f64) -> f32 {
    let anchor = item.last_accessed_at.unwrap_or(item.updated_at);
    half_life_decay(age_in_days(reference, anchor), half_life_days)
}

/// Normalize affect magnitude to `[0.0, 1.0]` for decay retention.
pub fn emotional_impact(affect: AffectAnnotation) -> f32 {
    let dist = affect.valence.hypot(affect.arousal);
    (dist / 2.83).clamp(0.0, 1.0)
}

/// Anchor for active-memory fade decisions: the last *content* update (#345).
///
/// Forgetting keys off `updated_at`, deliberately **not** `last_accessed_at`.
/// Recall bumps `last_accessed_at` on prompt inclusion, but that must not push
/// the decay anchor back to "now": otherwise a frequently-recalled memory could
/// never reach [`FADE_THRESHOLD`], and recall would simultaneously raise a
/// memory's score *and* shield it from forgetting — the self-reinforcing loop
/// this separation breaks. "Last recalled" and "last edited" are distinct
/// concepts; forgetting tracks the latter, recall recency
/// ([`recency_score`]) the former.
pub fn active_decay_anchor(item: &MemoryItem) -> DateTime<Utc> {
    item.updated_at
}

/// Anchor for faded-memory archive decisions (`faded_at → created_at`).
pub fn faded_decay_anchor(item: &MemoryItem) -> DateTime<Utc> {
    item.faded_at.unwrap_or(item.created_at)
}

/// Compute lifecycle retention score in `[0.0, 1.0]` (higher = retain longer).
///
/// Anchor depends on status: faded memories decay from `faded_at`, all others
/// from the active anchor. Pinned memories always return `1.0` and are exempt
/// from natural decay transitions.
pub fn decay_score(item: &MemoryItem, now: DateTime<Utc>, half_life_days: f64) -> f32 {
    if item.pinned {
        return 1.0;
    }

    let anchor = match item.status {
        MemoryStatus::Faded => faded_decay_anchor(item),
        _ => active_decay_anchor(item),
    };
    let base = half_life_decay(age_in_days(now, anchor), half_life_days);

    let salience_factor = 0.5f32.mul_add(item.salience.get(), 0.5);
    let confidence_factor = 0.5f32.mul_add(item.confidence.get(), 0.5);
    let emotional_factor = 0.3f32.mul_add(emotional_impact(item.affect), 0.7);

    (base * salience_factor * confidence_factor * emotional_factor).clamp(0.0, 1.0)
}

/// Return the natural-decay target status for a memory, if any.
pub fn target_status_after_decay(current: MemoryStatus, score: f32) -> Option<MemoryStatus> {
    match current {
        MemoryStatus::Active if score < FADE_THRESHOLD => Some(MemoryStatus::Faded),
        MemoryStatus::Faded if score < ARCHIVE_THRESHOLD => Some(MemoryStatus::Archived),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_core::{MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource};

    fn sample_item(status: MemoryStatus, pinned: bool, days_ago: i64) -> MemoryItem {
        let now = Utc::now();
        let anchor = now
            .checked_sub_signed(chrono::Duration::days(days_ago))
            .unwrap_or(now);
        MemoryItem {
            id: Some(1),
            scope: MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "test".into(),
            content: "content".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.5),
            salience: MemorySalience::new(0.5),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: anchor,
            updated_at: anchor,
            valid_from: None,
            valid_until: None,
            status,
            supersedes_id: None,
            pinned,
            faded_at: if status == MemoryStatus::Faded {
                Some(anchor)
            } else {
                None
            },
            commitment_id: None,
        }
    }

    #[test]
    fn half_life_decay_is_one_at_zero_age() {
        assert!((half_life_decay(0.0, 30.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn half_life_decay_is_half_at_one_half_life() {
        let v = half_life_decay(30.0, 30.0);
        assert!((v - 0.5).abs() < 1e-6);
    }

    #[test]
    fn half_life_decay_disables_on_nonpositive_or_nan() {
        assert!((half_life_decay(100.0, 0.0) - 1.0).abs() < f32::EPSILON);
        assert!((half_life_decay(100.0, -5.0) - 1.0).abs() < f32::EPSILON);
        assert!((half_life_decay(100.0, f64::NAN) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn half_life_decay_clamps_negative_age() {
        assert!((half_life_decay(-10.0, 30.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pinned_memory_decay_score_is_one() {
        let item = sample_item(MemoryStatus::Active, true, 365);
        let score = decay_score(&item, Utc::now(), 30.0);
        assert!((score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn decay_score_decreases_with_age() {
        let young = decay_score(
            &sample_item(MemoryStatus::Active, false, 1),
            Utc::now(),
            30.0,
        );
        let old = decay_score(
            &sample_item(MemoryStatus::Active, false, 120),
            Utc::now(),
            30.0,
        );
        assert!(young > old);
    }

    #[test]
    fn decay_score_salience_and_confidence_boost_retention() {
        let now = Utc::now();
        let mut low = sample_item(MemoryStatus::Active, false, 60);
        low.salience = MemorySalience::new(0.1);
        low.confidence = MemoryConfidence::new(0.1);

        let mut high = sample_item(MemoryStatus::Active, false, 60);
        high.salience = MemorySalience::new(0.95);
        high.confidence = MemoryConfidence::new(0.95);

        assert!(decay_score(&high, now, 30.0) > decay_score(&low, now, 30.0));
    }

    #[test]
    fn target_status_after_decay_thresholds() {
        assert_eq!(
            target_status_after_decay(MemoryStatus::Active, 0.39),
            Some(MemoryStatus::Faded)
        );
        assert_eq!(target_status_after_decay(MemoryStatus::Active, 0.41), None);
        assert_eq!(
            target_status_after_decay(MemoryStatus::Faded, 0.14),
            Some(MemoryStatus::Archived)
        );
        assert_eq!(target_status_after_decay(MemoryStatus::Faded, 0.20), None);
    }

    #[test]
    fn active_decay_anchor_ignores_last_accessed_at() {
        // #345: forgetting keys off content-update time, never recall time.
        let now = Utc::now();
        let mut item = sample_item(MemoryStatus::Active, false, 30);
        item.last_accessed_at = Some(now - chrono::Duration::days(5));
        assert_eq!(active_decay_anchor(&item), item.updated_at);
    }

    #[test]
    fn recalled_memory_still_fades_over_time() {
        // #345 regression: bumping `last_accessed_at` (as recall does) must not
        // reset the decay anchor, so a frequently-recalled-but-stale memory
        // still decays and can reach `FADE_THRESHOLD`.
        let now = Utc::now();
        let mut item = sample_item(MemoryStatus::Active, false, 120);
        // Simulate a recall that happened just now.
        item.last_accessed_at = Some(now);
        let score = decay_score(&item, now, 30.0);
        // 120 days = 4 half-lives of base decay (~0.0625) before retention
        // factors; a fresh anchor would score ~1.0. The recent access must not
        // rescue it.
        assert!(
            score < FADE_THRESHOLD,
            "a stale memory must fade despite a recent recall, got {score}"
        );
    }

    #[test]
    fn faded_decay_anchor_uses_faded_at_not_recent_updated_at() {
        let now = Utc::now();
        let old_fade = now - chrono::Duration::days(200);
        let mut item = sample_item(MemoryStatus::Faded, false, 200);
        item.faded_at = Some(old_fade);
        item.updated_at = now;
        let score = decay_score(&item, now, 30.0);
        let with_recent_updated = {
            let mut active_like = sample_item(MemoryStatus::Active, false, 0);
            active_like.updated_at = now;
            decay_score(&active_like, now, 30.0)
        };
        assert!(score < with_recent_updated);
    }

    #[test]
    fn faded_decay_anchor_falls_back_to_created_at() {
        let item = sample_item(MemoryStatus::Faded, false, 90);
        let mut legacy = item;
        legacy.faded_at = None;
        assert_eq!(faded_decay_anchor(&legacy), legacy.created_at);
    }
}
