//! Layer Composer: manages motion playback across body layers with
//! priority-based collision rules (#131).
//!
//! ## Layers
//!
//! | Layer | Scope | Coexistence |
//! |-------|-------|-------------|
//! | `Upper` | Upper-body gestures (arms, head, torso) | Coexists with Lower |
//! | `Lower` | Lower-body idle (legs, hips) | Coexists with Upper |
//! | `Full` | Full-body override | Preempts Upper and Lower |
//!
//! ## Collision rules
//!
//! - Full preempts everything (Upper and Lower are hidden while Full
//!   is active).
//! - Upper and Lower can play simultaneously.
//! - Same-layer replacement: higher priority replaces lower; equal
//!   priority = latest-wins.
//! - Expression weights use the same priority scheme, resolved per
//!   expression name.
//!
//! This module lives in `ene-vrm` and is independent of `ene-mind`.
//! The desktop app bridges `PerformanceCue` / `CueSource` from
//! `ene-mind` into the simple `(name, priority: u8)` inputs expected
//! here.

use std::collections::HashMap;

use crate::animation::{RepeatMode, VrmaPlayer};
use ene_config::MotionLayer;

/// A motion slot tracking a playing animation on one layer.
#[derive(Debug, Clone)]
struct MotionSlot {
    /// Motion / clip name (for lookup).
    pub name: String,
    /// Playback state.
    pub player: VrmaPlayer,
    /// Priority (higher = more important).
    pub priority: u8,
    /// Clip duration in seconds.
    pub duration: f32,
}

/// An expression entry with weight and priority.
#[derive(Debug, Clone)]
struct ExpressionEntry {
    pub weight: f32,
    pub priority: u8,
}

/// Composite frame result from the active layers.
#[derive(Debug, Clone, Default)]
pub struct ComposedFrame {
    /// Active motion names per layer.
    pub active_motions: Vec<String>,
    /// Expression weights (name → weight).
    pub expressions: HashMap<String, f32>,
    /// Whether Full layer is active (suppresses upper/lower).
    pub full_body_active: bool,
}

/// Manages motion playback across Upper/Lower/Full body layers
/// with priority-based collision resolution.
#[derive(Debug, Default)]
pub struct LayerComposer {
    upper: Option<MotionSlot>,
    lower: Option<MotionSlot>,
    full: Option<MotionSlot>,
    expressions: HashMap<String, ExpressionEntry>,
}

impl LayerComposer {
    /// Accept a motion cue, placing it on the appropriate layer with
    /// priority-based replacement.
    ///
    /// `priority` should follow the convention: 5 = command, 4 = advisory,
    /// 3 = affect, 2 = hysteresis, 1 = fallback.
    pub fn accept_motion(
        &mut self,
        name: String,
        layer: MotionLayer,
        priority: u8,
        duration: f32,
        repeat: RepeatMode,
    ) {
        let player = VrmaPlayer {
            repeat,
            ..VrmaPlayer::default()
        };
        let slot = MotionSlot {
            name,
            player,
            priority,
            duration,
        };

        match layer {
            MotionLayer::Upper => Self::place_slot(&mut self.upper, slot),
            MotionLayer::Lower => Self::place_slot(&mut self.lower, slot),
            MotionLayer::Full => Self::place_slot(&mut self.full, slot),
        }
    }

    /// Cancel a motion on a specific layer.
    pub fn cancel_motion(&mut self, layer: MotionLayer) {
        match layer {
            MotionLayer::Upper => self.upper = None,
            MotionLayer::Lower => self.lower = None,
            MotionLayer::Full => self.full = None,
        }
    }

    /// Cancel all motions across all layers.
    pub fn cancel_all_motions(&mut self) {
        self.upper = None;
        self.lower = None;
        self.full = None;
    }

    /// Set or update an expression weight with priority semantics.
    ///
    /// Higher-priority updates replace lower-priority ones for the
    /// same expression name.
    pub fn set_expression(&mut self, name: String, weight: f32, priority: u8) {
        let clamped = weight.clamp(0.0, 1.0);
        match self.expressions.get(&name) {
            Some(existing) if existing.priority <= priority => {
                self.expressions.insert(
                    name,
                    ExpressionEntry {
                        weight: clamped,
                        priority,
                    },
                );
            }
            None => {
                self.expressions.insert(
                    name,
                    ExpressionEntry {
                        weight: clamped,
                        priority,
                    },
                );
            }
            _ => {}
        }
    }

    /// Remove a single expression by name.
    pub fn remove_expression(&mut self, name: &str) {
        self.expressions.remove(name);
    }

    /// Clear all expressions.
    pub fn clear_expressions(&mut self) {
        self.expressions.clear();
    }

    /// Tick all active motion players by `dt` seconds.
    ///
    /// Full preempts Upper and Lower — when Full is active, only
    /// the Full slot advances and Upper/Lower clocks are paused.
    /// Once animations are auto-cleared when playback finishes.
    pub fn tick(&mut self, dt: f32) {
        if let Some(ref mut s) = self.full {
            if s.duration > 0.0 {
                s.player.advance(dt, s.duration);
            }
            if !s.player.playing {
                self.full = None;
            }
            return;
        }
        for slot in [&mut self.upper, &mut self.lower].into_iter().flatten() {
            if slot.duration > 0.0 {
                slot.player.advance(dt, slot.duration);
            }
        }
        if self.upper.as_ref().is_some_and(|s| !s.player.playing) {
            self.upper = None;
        }
        if self.lower.as_ref().is_some_and(|s| !s.player.playing) {
            self.lower = None;
        }
    }

    /// Returns the active motion names per layer (for the consumer to
    /// look up clips and evaluate frames).
    #[must_use]
    pub fn active_motion_names(&self) -> Vec<String> {
        if let Some(ref s) = self.full {
            return vec![s.name.clone()];
        }
        let mut names = Vec::with_capacity(2);
        if let Some(ref s) = self.upper {
            names.push(s.name.clone());
        }
        if let Some(ref s) = self.lower {
            names.push(s.name.clone());
        }
        names
    }

    /// Compose the current layer state into a [`ComposedFrame`].
    ///
    /// The consumer uses this to know which clips to evaluate and
    /// which expressions to apply.
    #[must_use]
    pub fn compose(&self) -> ComposedFrame {
        let full_body_active = self.full.is_some();
        let mut active_motions = Vec::with_capacity(2);

        if let Some(ref s) = self.full {
            active_motions.push(s.name.clone());
        } else {
            if let Some(ref s) = self.upper {
                active_motions.push(s.name.clone());
            }
            if let Some(ref s) = self.lower {
                active_motions.push(s.name.clone());
            }
        }

        let expressions: HashMap<String, f32> = self
            .expressions
            .iter()
            .map(|(k, v)| (k.clone(), v.weight))
            .collect();

        ComposedFrame {
            active_motions,
            expressions,
            full_body_active,
        }
    }

    /// Returns whether any motion is playing.
    #[must_use]
    pub const fn has_active_motion(&self) -> bool {
        self.upper.is_some() || self.lower.is_some() || self.full.is_some()
    }

    fn place_slot(target: &mut Option<MotionSlot>, slot: MotionSlot) {
        let replaced = match target {
            Some(existing) if existing.priority <= slot.priority => {
                let mut player = slot.player;
                player.play();
                *target = Some(MotionSlot { player, ..slot });
                true
            }
            None => {
                let mut player = slot.player;
                player.play();
                *target = Some(MotionSlot { player, ..slot });
                true
            }
            _ => false,
        };
        if replaced {
            tracing::debug!(
                component = "LayerComposer",
                name = target.as_ref().map(|s| s.name.as_str()),
                priority = target.as_ref().map(|s| s.priority),
                "Motion placed on layer"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_preempts_upper_and_lower() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3, 0.0, RepeatMode::Loop);
        lc.accept_motion("dance".into(), MotionLayer::Full, 5, 0.0, RepeatMode::Once);
        let frame = lc.compose();
        assert!(frame.full_body_active);
        assert_eq!(frame.active_motions, vec!["dance"]);
    }

    #[test]
    fn upper_and_lower_coexist() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3, 0.0, RepeatMode::Loop);
        let frame = lc.compose();
        assert!(!frame.full_body_active);
        assert_eq!(frame.active_motions.len(), 2);
        assert!(frame.active_motions.contains(&"wave".to_string()));
        assert!(frame.active_motions.contains(&"idle".to_string()));
    }

    #[test]
    fn higher_priority_replaces_same_layer() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 3, 0.0, RepeatMode::Once);
        lc.accept_motion("point".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["point"]);
    }

    #[test]
    fn lower_priority_does_not_replace_same_layer() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.accept_motion("point".into(), MotionLayer::Upper, 3, 0.0, RepeatMode::Once);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["wave"]);
    }

    #[test]
    fn equal_priority_latest_wins() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.accept_motion("point".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["point"]);
    }

    #[test]
    fn cancel_clears_layer() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.cancel_motion(MotionLayer::Upper);
        assert!(lc.compose().active_motions.is_empty());
    }

    #[test]
    fn cancel_all_clears_everything() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3, 0.0, RepeatMode::Loop);
        lc.cancel_all_motions();
        assert!(lc.compose().active_motions.is_empty());
    }

    #[test]
    fn expression_priority_merge() {
        let mut lc = LayerComposer::default();
        lc.set_expression("happy".into(), 0.8, 3);
        lc.set_expression("happy".into(), 1.0, 5);
        let frame = lc.compose();
        assert_eq!(frame.expressions.get("happy"), Some(&1.0));
    }

    #[test]
    fn lower_priority_expression_ignored() {
        let mut lc = LayerComposer::default();
        lc.set_expression("happy".into(), 1.0, 5);
        lc.set_expression("happy".into(), 0.3, 3);
        let frame = lc.compose();
        assert_eq!(frame.expressions.get("happy"), Some(&1.0));
    }

    #[test]
    fn expression_weight_clamped() {
        let mut lc = LayerComposer::default();
        lc.set_expression("happy".into(), 1.5, 5);
        lc.set_expression("sad".into(), -0.5, 5);
        let frame = lc.compose();
        assert_eq!(frame.expressions.get("happy"), Some(&1.0));
        assert_eq!(frame.expressions.get("sad"), Some(&0.0));
    }

    #[test]
    fn tick_advances_playing_motions() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 2.0, RepeatMode::Loop);
        lc.tick(0.5);
        assert!(lc.compose().active_motions.contains(&"wave".to_string()));
    }

    #[test]
    fn full_tick_preempts_upper() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3, 2.0, RepeatMode::Loop);
        lc.accept_motion("dance".into(), MotionLayer::Full, 5, 2.0, RepeatMode::Loop);
        lc.tick(0.5);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["dance"]);
    }

    #[test]
    fn once_motion_auto_clears() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 1.0, RepeatMode::Once);
        lc.tick(1.5);
        assert!(!lc.has_active_motion());
    }

    #[test]
    fn active_motion_names_full_preempts() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5, 0.0, RepeatMode::Once);
        lc.accept_motion("dance".into(), MotionLayer::Full, 5, 0.0, RepeatMode::Once);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["dance"]);
    }
}
