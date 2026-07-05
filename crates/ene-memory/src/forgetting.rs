//! Memory forgetting lifecycle: status transitions and decay scoring.
//!
//! Lifecycle [`decay_score`] is distinct from hybrid-recall [`crate::search::recency_score`]:
//! it drives post-turn `Active → Faded → Archived` transitions, while recall recency
//! only affects ranking among recallable rows.

use chrono::{DateTime, Utc};

use crate::typed_memory::{AffectAnnotation, MemoryItem, MemoryStatus};

/// Score below which an [`MemoryStatus::Active`] memory transitions to [`MemoryStatus::Faded`].
pub const FADE_THRESHOLD: f32 = 0.40;

/// Score below which a [`MemoryStatus::Faded`] memory transitions to [`MemoryStatus::Archived`].
pub const ARCHIVE_THRESHOLD: f32 = 0.15;

/// Error returned when a memory status transition is not allowed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid memory status transition: {from:?} -> {to:?}")]
pub struct InvalidTransition {
    /// Current status.
    pub from: MemoryStatus,
    /// Requested target status.
    pub to: MemoryStatus,
}

/// Validate whether a single-step lifecycle transition is permitted.
pub fn validate_transition(from: MemoryStatus, to: MemoryStatus) -> Result<(), InvalidTransition> {
    let allowed = matches!(
        (from, to),
        (MemoryStatus::Active, MemoryStatus::Faded)
            | (MemoryStatus::Active, MemoryStatus::Superseded)
            | (MemoryStatus::Active, MemoryStatus::UserDeleted)
            | (MemoryStatus::Active, MemoryStatus::Disputed)
            | (MemoryStatus::Faded, MemoryStatus::Archived)
            | (MemoryStatus::Faded, MemoryStatus::Disputed)
    );
    if allowed {
        Ok(())
    } else {
        Err(InvalidTransition { from, to })
    }
}

/// Normalize affect magnitude to `[0.0, 1.0]` for decay retention.
pub fn emotional_impact(affect: AffectAnnotation) -> f32 {
    let dist = (affect.valence * affect.valence + affect.arousal * affect.arousal).sqrt();
    (dist / 2.83).clamp(0.0, 1.0)
}

/// Anchor for active-memory fade decisions (`last_accessed_at → updated_at → created_at`).
#[must_use]
pub fn active_decay_anchor(item: &MemoryItem) -> DateTime<Utc> {
    item.last_accessed_at.unwrap_or(item.updated_at)
}

/// Anchor for faded-memory archive decisions.
#[must_use]
pub fn faded_decay_anchor(item: &MemoryItem) -> DateTime<Utc> {
    item.faded_at.unwrap_or(item.created_at)
}

/// Compute lifecycle retention score in `[0.0, 1.0]` (higher = retain longer).
///
/// Pinned memories always return `1.0` and are exempt from natural decay transitions.
pub fn decay_score(item: &MemoryItem, now: DateTime<Utc>, half_life_days: f64) -> f32 {
    if item.pinned {
        return 1.0;
    }

    let anchor = match item.status {
        MemoryStatus::Faded => faded_decay_anchor(item),
        _ => active_decay_anchor(item),
    };
    let age_secs = (now - anchor).num_seconds().max(0) as f64;
    let age_days = age_secs / 86_400.0;
    let base = if half_life_days <= 0.0 {
        1.0_f32
    } else {
        let lambda = std::f64::consts::LN_2 / half_life_days;
        (-lambda * age_days).exp() as f32
    };

    let salience_factor = 0.5 + 0.5 * item.salience.get();
    let confidence_factor = 0.5 + 0.5 * item.confidence.get();
    let emotional_factor = 0.7 + 0.3 * emotional_impact(item.affect);

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
    use crate::typed_memory::{
        MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource,
    };

    fn sample_item(status: MemoryStatus, pinned: bool, days_ago: i64) -> MemoryItem {
        let now = Utc::now();
        let anchor = now - chrono::Duration::days(days_ago);
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
        }
    }

    #[test]
    fn validate_transition_allows_issue_edges() {
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::Faded).is_ok());
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::Superseded).is_ok());
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::UserDeleted).is_ok());
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::Disputed).is_ok());
        assert!(validate_transition(MemoryStatus::Faded, MemoryStatus::Archived).is_ok());
        assert!(validate_transition(MemoryStatus::Faded, MemoryStatus::Disputed).is_ok());
    }

    #[test]
    fn validate_transition_rejects_invalid_edges() {
        assert!(validate_transition(MemoryStatus::Faded, MemoryStatus::Active).is_err());
        assert!(validate_transition(MemoryStatus::Archived, MemoryStatus::Faded).is_err());
        assert!(validate_transition(MemoryStatus::UserDeleted, MemoryStatus::Active).is_err());
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
    fn active_decay_anchor_prefers_last_accessed_at() {
        let now = Utc::now();
        let mut item = sample_item(MemoryStatus::Active, false, 30);
        item.last_accessed_at = Some(now - chrono::Duration::days(5));
        assert_eq!(active_decay_anchor(&item), item.last_accessed_at.unwrap());
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
        let mut legacy = item.clone();
        legacy.faded_at = None;
        assert_eq!(faded_decay_anchor(&legacy), legacy.created_at);
    }
}
