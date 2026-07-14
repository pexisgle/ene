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

use crate::animation::VrmaPlayer;

/// Motion body layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MotionLayer {
    /// Upper-body gesture + expression (arms, head, torso).
    Upper,
    /// Lower-body idle loop (legs, hips).
    Lower,
    /// Full-body override — preempts upper and lower.
    Full,
}

/// A motion slot tracking a playing animation on one layer.
#[derive(Debug, Clone)]
struct MotionSlot {
    /// Motion / clip name (for lookup).
    pub name: String,
    /// Playback state.
    pub player: VrmaPlayer,
    /// Priority (higher = more important).
    pub priority: u8,
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
    pub fn accept_motion(&mut self, name: String, layer: MotionLayer, priority: u8) {
        let slot = MotionSlot {
            name,
            player: VrmaPlayer::default(),
            priority,
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
    /// `clip_durations` maps clip name → duration in seconds.
    pub fn tick(&mut self, dt: f32, clip_durations: &HashMap<String, f32>) {
        for slot in [&mut self.upper, &mut self.lower, &mut self.full]
            .into_iter()
            .flatten()
        {
            let duration = clip_durations.get(&slot.name).copied().unwrap_or(0.0);
            if duration > 0.0 {
                slot.player.advance(dt, duration);
            }
        }
    }

    /// Returns the active motion names per layer (for the consumer to
    /// look up clips and evaluate frames).
    #[must_use]
    pub fn active_motion_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(2);
        if let Some(ref s) = self.upper {
            names.push(s.name.clone());
        }
        if let Some(ref s) = self.lower {
            names.push(s.name.clone());
        }
        if let Some(ref s) = self.full {
            names.push(s.name.clone());
        }
        names
    }

    /// Returns a reference to the Full-layer player, if active.
    #[must_use]
    pub fn full_player(&self) -> Option<&VrmaPlayer> {
        self.full.as_ref().map(|s| &s.player)
    }

    /// Returns a reference to the Upper-layer player, if active.
    #[must_use]
    pub fn upper_player(&self) -> Option<&VrmaPlayer> {
        self.upper.as_ref().map(|s| &s.player)
    }

    /// Returns a reference to the Lower-layer player, if active.
    #[must_use]
    pub fn lower_player(&self) -> Option<&VrmaPlayer> {
        self.lower.as_ref().map(|s| &s.player)
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
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3);
        lc.accept_motion("dance".into(), MotionLayer::Full, 5);
        let frame = lc.compose();
        assert!(frame.full_body_active);
        assert_eq!(frame.active_motions, vec!["dance"]);
    }

    #[test]
    fn upper_and_lower_coexist() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3);
        let frame = lc.compose();
        assert!(!frame.full_body_active);
        assert_eq!(frame.active_motions.len(), 2);
        assert!(frame.active_motions.contains(&"wave".to_string()));
        assert!(frame.active_motions.contains(&"idle".to_string()));
    }

    #[test]
    fn higher_priority_replaces_same_layer() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 3);
        lc.accept_motion("point".into(), MotionLayer::Upper, 5);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["point"]);
    }

    #[test]
    fn lower_priority_does_not_replace_same_layer() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        lc.accept_motion("point".into(), MotionLayer::Upper, 3);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["wave"]);
    }

    #[test]
    fn equal_priority_latest_wins() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        lc.accept_motion("point".into(), MotionLayer::Upper, 5);
        let names = lc.active_motion_names();
        assert_eq!(names, vec!["point"]);
    }

    #[test]
    fn cancel_clears_layer() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        lc.cancel_motion(MotionLayer::Upper);
        assert!(lc.compose().active_motions.is_empty());
    }

    #[test]
    fn cancel_all_clears_everything() {
        let mut lc = LayerComposer::default();
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        lc.accept_motion("idle".into(), MotionLayer::Lower, 3);
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
        lc.accept_motion("wave".into(), MotionLayer::Upper, 5);
        let mut durations = HashMap::new();
        durations.insert("wave".to_string(), 2.0);
        lc.tick(0.5, &durations);
        let player = lc.upper_player().unwrap();
        assert!((player.time - 0.5).abs() < 1e-5);
        assert!(player.playing);
    }
}
