//! Beat-synced procedural body sway for the avatar.
//!
//! Hosts feed detected system-audio beats as `{ bpm, intensity }` pulses
//! ([`BeatSway::on_pulse`]); the sway keeps its own clock so it stays
//! beat-locked between pulses and decays to rest once the music stops.
//! Composed additively onto the VRMA pose ([`BeatSway::apply_to`]), so it
//! works with or without motion assets.

use std::f32::consts::TAU;

use glam::Quat;

use crate::animation::VrmaFrame;

/// Peak rotation amplitude at full intensity, in radians (~3.4 degrees).
///
/// Kept deliberately small: the sway is a subtle dance accent layered under
/// performance-cue motions, not a replacement for them.
const MAX_AMPLITUDE_RAD: f32 = 0.06;

/// How long a beat keeps the sway (and locomotion speed sync) active.
const PULSE_TAIL_SECS: f32 = 1.5;

/// Exponential intensity decay rate per second (falls to ~5% in 1 s).
const INTENSITY_DECAY_PER_SEC: f32 = 3.0;

/// Tempo the locomotion speed multiplier treats as "normal".
const REFERENCE_BPM: f32 = 120.0;

/// Minimum playback speed when the detected tempo is slower than reference.
const MIN_SPEED: f32 = 0.85;

/// Maximum playback speed when the detected tempo is faster than reference.
const MAX_SPEED: f32 = 1.2;

/// Stateful beat-locked sway driven by `{ bpm, intensity }` pulses.
#[derive(Debug, Clone)]
pub struct BeatSway {
    bpm: f32,
    intensity: f32,
    phase: f32,
    since_pulse: f32,
}

impl Default for BeatSway {
    fn default() -> Self {
        Self {
            bpm: REFERENCE_BPM,
            intensity: 0.0,
            phase: 0.0,
            since_pulse: f32::MAX,
        }
    }
}

impl BeatSway {
    /// Register a detected beat.
    ///
    /// The phase snaps back to zero so the sway's peak lands on the beat;
    /// the snap coincides with the intensity attack, which masks the small
    /// rotational jump. `bpm` is clamped to a plausible tempo range and
    /// `intensity` to `[0, 1]` so a misbehaving host cannot blow up the
    /// amplitude.
    pub fn on_pulse(&mut self, bpm: f32, intensity: f32) {
        self.bpm = bpm.clamp(30.0, 300.0);
        self.intensity = intensity.clamp(0.0, 1.0);
        self.phase = 0.0;
        self.since_pulse = 0.0;
    }

    /// Advance the sway clock by `dt_secs`.
    ///
    /// The phase advances at the current BPM; the intensity decays toward
    /// zero so the body settles between beats and fully rests after the
    /// pulse tail.
    pub fn update(&mut self, dt_secs: f32) {
        self.since_pulse += dt_secs;
        if self.since_pulse > PULSE_TAIL_SECS {
            self.intensity = 0.0;
            return;
        }
        self.phase = (self.phase + dt_secs * (self.bpm / 60.0) * TAU) % TAU;
        self.intensity *= (-INTENSITY_DECAY_PER_SEC * dt_secs).exp();
    }

    /// Whether a recent beat still drives motion.
    pub fn is_active(&self) -> bool {
        self.since_pulse <= PULSE_TAIL_SECS && self.intensity > 0.001
    }

    /// Playback-speed multiplier for locomotion clips.
    ///
    /// Scales the clip toward the reference tempo (120 BPM); returns `1.0`
    /// once the sway tail expires so a dead beat never freezes the avatar
    /// at the last detected tempo.
    pub fn locomotion_speed_multiplier(&self) -> f32 {
        if !self.is_active() {
            return 1.0;
        }
        (self.bpm / REFERENCE_BPM).clamp(MIN_SPEED, MAX_SPEED)
    }

    /// Compose the sway onto `frame` as additive bone rotations.
    ///
    /// Bones already posed by the clip keep their rotation (sway multiplies
    /// on top); unposed bones gain a sway-only rotation, so the reaction
    /// works without any motion asset.
    pub fn apply_to(&self, frame: &mut VrmaFrame) {
        if !self.is_active() {
            return;
        }
        let amp = MAX_AMPLITUDE_RAD * self.intensity;
        let phase_sin = self.phase.sin();
        let double_phase_sin = (2.0 * self.phase).sin();

        let hips = Quat::from_rotation_y(amp * phase_sin)
            * Quat::from_rotation_z(amp * 0.35 * double_phase_sin);
        let spine = Quat::from_rotation_z(amp * 0.3 * (self.phase + 0.2).sin());
        let chest = Quat::from_rotation_z(amp * 0.7 * (self.phase + 0.1).sin())
            * Quat::from_rotation_y(amp * 0.4 * double_phase_sin);
        let head = Quat::from_rotation_y(amp * -0.5 * phase_sin)
            * Quat::from_rotation_x(amp * 0.25 * (2.0 * self.phase + 0.3).sin());

        for (bone, sway) in [
            ("hips", hips),
            ("spine", spine),
            ("chest", chest),
            ("head", head),
        ] {
            let entry = frame
                .bone_rotations
                .entry(bone.to_string())
                .or_insert(Quat::IDENTITY);
            *entry = sway * *entry;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sway_is_inactive_and_leaves_frame_untouched() {
        let sway = BeatSway::default();
        assert!(!sway.is_active());
        assert!((sway.locomotion_speed_multiplier() - 1.0).abs() < 1e-5);
        let mut frame = VrmaFrame::default();
        sway.apply_to(&mut frame);
        assert!(frame.bone_rotations.is_empty());
    }

    #[test]
    fn pulse_activates_sway_and_snaps_phase_to_beat() {
        let mut sway = BeatSway::default();
        sway.on_pulse(120.0, 1.0);
        assert!(sway.is_active());
        sway.update(0.25);
        // 0.25 s at 120 BPM = half a beat = pi radians.
        assert!((sway.phase - std::f32::consts::PI).abs() < 1e-4);
    }

    #[test]
    fn sway_fades_out_after_pulse_tail() {
        let mut sway = BeatSway::default();
        sway.on_pulse(120.0, 1.0);
        for _ in 0..20 {
            sway.update(0.1);
        }
        assert!(!sway.is_active());
        assert!((sway.locomotion_speed_multiplier() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn sway_composes_with_existing_pose_and_inserts_missing_bones() {
        let mut sway = BeatSway::default();
        sway.on_pulse(120.0, 1.0);
        let mut frame = VrmaFrame::default();
        let posed = Quat::from_rotation_x(0.1);
        frame.bone_rotations.insert("chest".to_string(), posed);
        sway.apply_to(&mut frame);
        assert!(frame.bone_rotations.contains_key("hips"));
        assert!(frame.bone_rotations.contains_key("head"));
        let chest = frame.bone_rotations["chest"];
        assert_ne!(chest, posed, "sway must change the posed chest rotation");
    }

    #[test]
    fn locomotion_speed_scales_toward_reference_bpm() {
        let mut sway = BeatSway::default();
        sway.on_pulse(180.0, 1.0);
        assert!((sway.locomotion_speed_multiplier() - 1.2).abs() < 1e-5);
        sway.on_pulse(60.0, 1.0);
        assert!((sway.locomotion_speed_multiplier() - 0.85).abs() < 1e-5);
    }

    #[test]
    fn amplitude_is_clamped_and_bounded() {
        let mut sway = BeatSway::default();
        sway.on_pulse(120.0, 10.0);
        assert!(sway.is_active());
        let mut frame = VrmaFrame::default();
        sway.apply_to(&mut frame);
        for quat in frame.bone_rotations.values() {
            assert!(
                quat.to_axis_angle().1 <= MAX_AMPLITUDE_RAD * 1.5,
                "rotation angle {:?} exceeds the bounded amplitude",
                quat.to_axis_angle().1
            );
        }
    }
}
