//! Topic-boundary detection via a topic centroid and a composite score (#367).
//!
//! A naive "cosine similarity between two consecutive utterances" fails for
//! three reasons: backchannels ("うん", "そうだね") are dissimilar to the
//! surrounding turn yet stay on-topic, very short utterances have unstable
//! embeddings, and topics drift gradually so no single low-similarity pair
//! ever appears. This module instead maintains a **topic centroid** — an
//! exponentially weighted moving average of the embeddings that belong to the
//! current topic — and scores each completed turn with a composite of:
//!
//! 1. the cosine distance of the turn's embedding from the centroid (primary),
//! 2. the silence elapsed since the previous utterance, and
//! 3. how long the current topic has run (turn count).
//!
//! The turn-count term is the accumulator that makes *gradual* drift detectable:
//! a centroid that tracks its topic closely keeps the distance term small, but
//! an over-long topic is flagged through its turn count alone. Short utterances
//! (below [`TopicBoundaryConfig::min_utterance_chars`]) are excluded from both
//! scoring and centroid updates so a backchannel cannot corrupt the centroid.
//!
//! This module only *detects* a boundary and produces a [`TopicBoundarySignal`].
//! Acting on the signal — retrospective compression (#368) and session splitting
//! (#369) — is the responsibility of later stages.

use crate::config::TopicBoundaryConfig;
use chrono::{DateTime, Utc};
use ene_ai::cosine_similarity;

/// Running state of the topic-boundary detector for a single session (#367).
///
/// Stored on the session so it survives across turns (the streaming path
/// commits the session back to the actor at the end of every turn). The
/// centroid is the exponentially weighted moving average of the embeddings
/// that belong to the current topic; `turns_in_topic` counts completed turns
/// since the topic last reset and drives the over-long-topic signal.
#[derive(Clone, Debug, Default)]
pub struct TopicBoundaryTracker {
    /// Centroid (moving average) of the current topic's embeddings.
    centroid: Option<Vec<f32>>,
    /// Completed turns accumulated under the current topic.
    turns_in_topic: usize,
    /// When the most recent qualifying utterance arrived, used to measure the
    /// silence elapsed since it.
    last_utterance_at: Option<DateTime<Utc>>,
}

/// The outcome of scoring one completed turn against the topic centroid (#367).
///
/// `score` is the composite boundary score in `0.0..=1.0`; `boundary` is true
/// once it reaches the configured threshold. Compression (#368) consumes the
/// score directly; session split (#369) does not use topic boundaries.
#[derive(Clone, Debug)]
pub struct TopicBoundarySignal {
    /// Composite boundary score, clamped to `0.0..=1.0`.
    pub score: f32,
    /// Whether the score reached the configured boundary threshold.
    pub boundary: bool,
    /// Cosine distance (`1.0 - similarity`) of the turn from the centroid.
    pub centroid_distance: f32,
    /// Normalized silence contribution (`0.0..=1.0`).
    pub silence_factor: f32,
    /// Normalized topic-length contribution (`0.0..=1.0`).
    pub topic_length_factor: f32,
}

impl TopicBoundarySignal {
    /// A neutral signal: no boundary, zero score. Returned when detection is
    /// disabled, the turn has no usable embedding, or the utterance is a
    /// backchannel below the length floor (#367).
    #[must_use]
    pub const fn none() -> Self {
        Self {
            score: 0.0,
            boundary: false,
            centroid_distance: 0.0,
            silence_factor: 0.0,
            topic_length_factor: 0.0,
        }
    }
}

impl TopicBoundaryTracker {
    /// Creates an empty tracker with no established topic.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            centroid: None,
            turns_in_topic: 0,
            last_utterance_at: None,
        }
    }

    /// Number of completed turns accumulated under the current topic.
    #[must_use]
    pub const fn turns_in_topic(&self) -> usize {
        self.turns_in_topic
    }

    /// Whether a topic centroid has been established yet.
    #[must_use]
    pub const fn has_centroid(&self) -> bool {
        self.centroid.is_some()
    }

    /// Discards the centroid and turn count, starting a fresh topic (#367).
    ///
    /// Called when a session split commits (#369) or when a boundary is
    /// confirmed and the new topic begins. Compression (#368) must **not**
    /// call this: compression is a physical operation, not a topic boundary,
    /// so the centroid survives it.
    pub fn reset_topic(&mut self) {
        self.centroid = None;
        self.turns_in_topic = 0;
    }

    /// Scores one completed turn and updates the running topic state (#367).
    ///
    /// `embedding` is the turn's input embedding (the session's
    /// `last_input_embedding`, consumed here), `utterance_chars` is the length
    /// of the user utterance in characters, and `now` is the evaluation time
    /// (turn completion). The returned signal always reflects the composite
    /// score; when `boundary` is true the tracker resets the topic and seeds
    /// the centroid with this turn's embedding, so the new topic starts from
    /// the utterance that crossed the boundary.
    ///
    /// Utterances shorter than [`TopicBoundaryConfig::min_utterance_chars`]
    /// are treated as backchannels: they neither score a boundary nor update
    /// the centroid, so a "うん" cannot corrupt the topic model.
    pub fn observe_turn(
        &mut self,
        config: &TopicBoundaryConfig,
        embedding: &[f32],
        utterance_chars: usize,
        now: DateTime<Utc>,
    ) -> TopicBoundarySignal {
        if !config.enabled || embedding.is_empty() || utterance_chars < config.min_utterance_chars {
            return TopicBoundarySignal::none();
        }

        // A dimension change (the embedding provider was swapped) makes the
        // running centroid incomparable; reseed and treat this turn as a fresh
        // topic start rather than signalling a spurious boundary.
        if self
            .centroid
            .as_ref()
            .is_some_and(|c| c.len() != embedding.len())
        {
            self.seed_new_topic(embedding, now);
            return TopicBoundarySignal::none();
        }

        let centroid_distance = self.centroid.as_ref().map_or(0.0, |centroid| {
            (1.0 - cosine_similarity(centroid, embedding)).clamp(0.0, 1.0)
        });
        let silence_factor = self.silence_factor(config, now);
        let topic_length_factor = topic_length_factor(self.turns_in_topic, config);

        let score = (config.weight_centroid * centroid_distance
            + config.weight_silence * silence_factor
            + config.weight_topic_length * topic_length_factor)
            .clamp(0.0, 1.0);
        let boundary = score >= config.boundary_threshold;

        if boundary {
            // The utterance that crossed the boundary opens the new topic.
            self.seed_new_topic(embedding, now);
        } else {
            self.update_centroid(embedding, config);
            self.turns_in_topic = self.turns_in_topic.saturating_add(1);
            self.last_utterance_at = Some(now);
        }

        TopicBoundarySignal {
            score,
            boundary,
            centroid_distance,
            silence_factor,
            topic_length_factor,
        }
    }

    /// Starts a fresh topic seeded from `embedding` (#367).
    fn seed_new_topic(&mut self, embedding: &[f32], now: DateTime<Utc>) {
        self.centroid = Some(embedding.to_vec());
        self.turns_in_topic = 1;
        self.last_utterance_at = Some(now);
    }

    /// Normalized silence contribution: elapsed time since the previous
    /// qualifying utterance divided by the saturation window.
    fn silence_factor(&self, config: &TopicBoundaryConfig, now: DateTime<Utc>) -> f32 {
        let Some(prev) = self.last_utterance_at else {
            return 0.0;
        };
        let elapsed_secs = now.signed_duration_since(prev).num_seconds().max(0);
        let saturation = f64::from(config.silence_saturation_secs.max(1));
        ((elapsed_secs as f64 / saturation) as f32).clamp(0.0, 1.0)
    }

    /// Folds `embedding` into the centroid as an exponentially weighted moving
    /// average. The first qualifying utterance seeds the centroid verbatim.
    /// Callers must ensure `embedding` matches the centroid's dimension (a
    /// mismatch is handled upstream as a fresh topic start).
    fn update_centroid(&mut self, embedding: &[f32], config: &TopicBoundaryConfig) {
        let Some(centroid) = self.centroid.as_mut() else {
            self.centroid = Some(embedding.to_vec());
            return;
        };
        let alpha = config.centroid_alpha.clamp(0.0, 1.0);
        for (slot, value) in centroid.iter_mut().zip(embedding) {
            *slot = (1.0 - alpha).mul_add(*slot, alpha * value);
        }
    }
}

/// Normalized topic-length contribution: turns in the current topic divided by
/// the soft cap, saturating at `1.0`. An over-long topic is likely to contain
/// multiple topics, so this term accumulates even when the centroid keeps
/// tracking the drift closely — this is what makes gradual drift detectable.
///
/// The term alone can never cross the boundary: [`TopicBoundaryConfig`]
/// requires `weight_topic_length < boundary_threshold` (the default 0.25 vs
/// 0.30), so the length component only pushes an already-suspicious topic
/// (drift or silence) over the line instead of force-splitting every coherent
/// topic at the cap.
fn topic_length_factor(turns_in_topic: usize, config: &TopicBoundaryConfig) -> f32 {
    let cap = config.max_topic_turns.max(1) as f32;
    (turns_in_topic as f32 / cap).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "tests index into fixed-size fixture vectors"
    )]
    use super::*;

    /// A unit vector along axis `i` in a `dim`-dimensional space.
    fn axis(dim: usize, i: usize) -> Vec<f32> {
        let mut v = vec![0.0; dim];
        v[i] = 1.0;
        v
    }

    /// A unit vector that is `t` of the way from axis `from` toward axis `to`.
    fn blend(dim: usize, from: usize, to: usize, t: f32) -> Vec<f32> {
        let a = axis(dim, from);
        let b = axis(dim, to);
        let mut v = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (1.0 - t).mul_add(*x, t * y))
            .collect::<Vec<_>>();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v {
            *x /= norm;
        }
        v
    }

    fn config() -> TopicBoundaryConfig {
        TopicBoundaryConfig {
            enabled: true,
            ..TopicBoundaryConfig::default()
        }
    }

    fn t0() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    #[test]
    fn first_utterance_seeds_centroid_without_boundary() {
        let mut tracker = TopicBoundaryTracker::new();
        let signal = tracker.observe_turn(&config(), &axis(4, 0), 40, t0());

        assert!(!signal.boundary);
        assert!(tracker.has_centroid());
        assert_eq!(tracker.turns_in_topic(), 1);
        // No prior centroid or utterance, so distance and silence are zero.
        assert!(signal.centroid_distance < f32::EPSILON);
        assert!(signal.silence_factor < f32::EPSILON);
    }

    #[test]
    fn short_backchannel_is_ignored_and_does_not_move_centroid() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = config();
        let topic = axis(4, 0);
        tracker.observe_turn(&cfg, &topic, 40, t0());
        let centroid_before = tracker.centroid.clone().expect("seeded");

        // "うん" — far from the topic but below the length floor.
        let signal = tracker.observe_turn(&cfg, &axis(4, 3), 2, t0());

        assert!(!signal.boundary);
        assert!(signal.score < f32::EPSILON);
        assert_eq!(tracker.centroid, Some(centroid_before));
        // A backchannel does not count as a topic turn.
        assert_eq!(tracker.turns_in_topic(), 1);
    }

    #[test]
    fn abrupt_topic_change_is_detected() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = config();
        // Several on-topic turns anchor the centroid near axis 0.
        for _ in 0..3 {
            tracker.observe_turn(&cfg, &axis(4, 0), 40, t0());
        }
        assert!(!tracker.observe_turn(&cfg, &axis(4, 0), 40, t0()).boundary);

        // A sudden jump to an orthogonal topic crosses the threshold.
        let signal = tracker.observe_turn(&cfg, &axis(4, 1), 40, t0());
        assert!(signal.boundary);
        assert!(signal.score >= cfg.boundary_threshold);
        // The new topic restarts from the crossing utterance.
        assert_eq!(tracker.turns_in_topic(), 1);
    }

    #[test]
    fn gradual_drift_is_detected_via_turn_count() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = config();
        let dim = 4;
        // Drift slowly from axis 0 toward axis 1 across a full `max_topic_turns`
        // topic. Each step stays close enough to the (tracking) centroid that
        // the distance term alone never trips the threshold — the lag between
        // the centroid and the drifting input yields a cosine distance of
        // roughly 0.26 here, far below the 0.30 threshold. The accumulating
        // turn-count term provides the remaining push, so the boundary fires at
        // the soft cap. A naive consecutive-pair similarity check would miss
        // this: adjacent utterances are ~0.99 similar.
        let steps = cfg.max_topic_turns;
        let mut detected_at = None;
        let mut peak_centroid_term = 0.0_f32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let emb = blend(dim, 0, 1, t);
            let signal = tracker.observe_turn(&cfg, &emb, 40, t0());
            if signal.boundary {
                detected_at = Some(i);
                assert!(
                    signal.topic_length_factor > signal.centroid_distance,
                    "gradual drift should be caught by the turn-count term"
                );
                break;
            }
            peak_centroid_term =
                peak_centroid_term.max(cfg.weight_centroid * signal.centroid_distance);
        }
        assert!(
            peak_centroid_term < cfg.boundary_threshold,
            "the distance term alone never crosses the threshold — the turn count provides the final push"
        );
        assert!(
            detected_at.is_some_and(|i| i >= steps.saturating_sub(4)),
            "gradual drift should cross the threshold only as the turn-count term accumulates, got {detected_at:?}"
        );
    }

    #[test]
    fn stable_topic_never_fires_boundary_at_the_soft_cap() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = config();
        let topic = axis(4, 0);
        // A perfectly coherent topic: identical utterances, no silence. The
        // turn-count term saturates at the soft cap, but with
        // `weight_topic_length < boundary_threshold` (the documented invariant)
        // it can never cross the threshold on its own (#367).
        let mut seen = 0;
        for _ in 0..=cfg.max_topic_turns.saturating_mul(2) {
            let signal = tracker.observe_turn(&cfg, &topic, 40, t0());
            assert!(!signal.boundary);
            seen += 1;
            if seen > cfg.max_topic_turns {
                // Past the soft cap the term is saturated, yet the score must
                // stay below the threshold — the invariant that makes the cap
                // soft rather than a hard force-split.
                assert!(signal.topic_length_factor > 0.99);
            }
        }
        assert!(seen > cfg.max_topic_turns, "test must exceed the soft cap");
    }

    #[test]
    fn long_silence_raises_boundary_score() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = config();
        let topic = axis(4, 0);
        tracker.observe_turn(&cfg, &topic, 40, t0());

        // A somewhat-off-topic utterance after a long silence scores higher
        // than the same utterance with no silence.
        let off = blend(4, 0, 1, 0.5);
        let mut quiet = TopicBoundaryTracker::new();
        quiet.observe_turn(&cfg, &topic, 40, t0());
        let no_silence = quiet.observe_turn(&cfg, &off, 40, t0());

        let after_silence = tracker.observe_turn(
            &cfg,
            &off,
            40,
            t0() + chrono::Duration::seconds(i64::from(cfg.silence_saturation_secs)),
        );
        assert!(after_silence.silence_factor > 0.99);
        assert!(after_silence.score > no_silence.score);
    }

    #[test]
    fn disabled_config_never_signals() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = TopicBoundaryConfig {
            enabled: false,
            ..TopicBoundaryConfig::default()
        };
        for _ in 0..50 {
            let signal = tracker.observe_turn(&cfg, &axis(4, 0), 40, t0());
            assert!(!signal.boundary);
            assert!(signal.score < f32::EPSILON);
        }
        assert!(!tracker.has_centroid());
    }

    #[test]
    fn reset_topic_clears_centroid_and_count() {
        let mut tracker = TopicBoundaryTracker::new();
        tracker.observe_turn(&config(), &axis(4, 0), 40, t0());
        assert!(tracker.has_centroid());

        tracker.reset_topic();
        assert!(!tracker.has_centroid());
        assert_eq!(tracker.turns_in_topic(), 0);
    }

    #[test]
    fn dimension_change_reseeds_centroid() {
        let mut tracker = TopicBoundaryTracker::new();
        let cfg = config();
        tracker.observe_turn(&cfg, &axis(4, 0), 40, t0());

        // A different-dimension embedding cannot be averaged in; reseed.
        let signal = tracker.observe_turn(&cfg, &axis(8, 0), 40, t0());
        assert!(!signal.boundary);
        assert_eq!(tracker.centroid.as_ref().map(Vec::len), Some(8));
    }
}
