//! VRMC_vrm_animation-1.0 (VRMA) parser, playback engine, and retargeting.
//!
//! Loads `.vrma` files (glTF binary with the
//! `VRMC_vrm_animation` extension) and provides a per-frame
//! evaluator that samples bone rotations, expression weights,
//! and look-at gaze direction at an arbitrary time.
//!
//! ## Spec background
//!
//! A VRMA file is a standard glTF/GLB that carries:
//!
//! 1. A **node hierarchy** defining a virtual skeleton.
//! 2. A root-level `VRMC_vrm_animation` extension that maps
//!    semantic names (humanoid bones, expressions, look-at)
//!    to glTF node indices.
//! 3. Standard glTF `animations[]` whose channels target
//!    those nodes.
//!
//! The extension is purely a **semantic mapping layer** — it
//! does not define its own clip/track/sampler types. All
//! animation data (keyframes, interpolation) comes from glTF
//! core.
//!
//! ## Track types
//!
//! | Target | glTF path | Semantics |
//! |--------|-----------|-----------|
//! | Humanoid bone | `rotation` | Local rotation (quat) |
//! | Hips bone | `translation` | Local position (vec3) |
//! | Expression | `translation` | `x` component = weight `[0, 1]` |
//! | Look-at | `rotation` | Yaw/pitch via Extrinsic ZXY |
//!
//! ## Retargeting
//!
//! Different VRM models have different T-poses (rest rotations).
//! The VRM spec defines a two-step normalization:
//!
//! ```text
//! NormalizedLocalRotation = W_src * L_src^-1 * Pose * W_src^-1
//! PoseForDst = L_dst * W_dst^-1 * NormalizedLocalRotation * W_dst
//! ```
//!
//! where `W` is the world rest rotation and `L` is the local
//! rest rotation. Hips translation uses height-based scaling:
//! `delta * (dst_hips_height / src_hips_height)`.
//!
//! ## Conventions
//!
//! - Expression weights are clamped to `[0, 1]`.
//! - The first animation in `animations[]` is loaded by default.
//! - Scale is forbidden on humanoid bones and ignored if present.
use std::collections::HashMap;
use std::path::Path;

use glam::{Quat, Vec3};

use crate::error::{VrmError, VrmResult};
use crate::humanoid::{HumanoidBoneRegistry, VrmBone, canonicalize_bone_name};

/// Interpolation mode for keyframe sampling. Mirrors the glTF
/// `sampler.interpolation` enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Interpolation {
    /// Hold the previous keyframe value until the next.
    Step,
    /// Linear interpolation (slerp for quaternions).
    #[default]
    Linear,
    /// Cubic Hermite spline with in-tangent, value, out-tangent.
    CubicSpline,
}

impl Interpolation {
    const fn from_gltf(mode: gltf::animation::Interpolation) -> Self {
        match mode {
            gltf::animation::Interpolation::Step => Self::Step,
            gltf::animation::Interpolation::Linear => Self::Linear,
            gltf::animation::Interpolation::CubicSpline => Self::CubicSpline,
        }
    }
}

/// A keyframe sampler: timestamps + values + interpolation mode.
///
/// For `CubicSpline`, `values` stores 3 entries per keyframe
/// (in-tangent, value, out-tangent) in that order, matching the
/// glTF spec layout.
#[derive(Clone, Debug)]
pub struct Sampler<T: Clone> {
    /// Monotonically increasing timestamps in seconds.
    pub times: Vec<f32>,
    /// Keyframe values. For `CubicSpline`, the length is
    /// `3 * times.len()` (in-tangent, value, out-tangent per key).
    pub values: Vec<T>,
    pub interpolation: Interpolation,
}

impl<T: Clone> Sampler<T> {
    /// Duration of this sampler (last timestamp, or 0 if empty).
    pub fn duration(&self) -> f32 {
        self.times.last().copied().unwrap_or(0.0)
    }

    pub const fn keyframe_count(&self) -> usize {
        self.times.len()
    }
}

/// Bone animation channel: rotation + optional translation (hips only).
#[derive(Clone, Debug)]
pub struct BoneChannel {
    /// Bone name (canonical, e.g. `"hips"`, `"leftUpperArm"`).
    pub bone_name: String,
    /// Rotation keyframes (quaternion, xyzw).
    pub rotation: Sampler<Quat>,
    /// Translation keyframes (vec3). Only present for the `hips` bone.
    pub translation: Option<Sampler<Vec3>>,
}

/// Expression animation channel. The weight is sampled from
/// `translation.x` of the expression's glTF node, clamped to `[0, 1]`.
#[derive(Clone, Debug)]
pub struct ExpressionChannel {
    /// Expression name (e.g. `"happy"`, `"blink"`, `"aa"`).
    pub name: String,
    /// Weight keyframes (scalar, from `translation.x`).
    pub weight: Sampler<f32>,
}

/// Look-at animation channel. The rotation is decomposed into
/// yaw/pitch using Extrinsic ZXY Euler angles.
#[derive(Clone, Debug)]
pub struct LookAtChannel {
    /// Rotation keyframes (quaternion).
    pub rotation: Sampler<Quat>,
    /// Offset from head bone to eye gaze origin.
    pub offset_from_head_bone: [f32; 3],
}

/// A single VRMA animation clip. Contains all channels that
/// target nodes defined in the `VRMC_vrm_animation` extension.
#[derive(Clone, Debug)]
pub struct VrmaClip {
    /// Clip name (from glTF `animation.name`, or `"clip_<i>"`).
    pub name: String,
    /// Total duration in seconds (max of all channel durations).
    pub duration: f32,
    /// Bone animation channels, keyed by canonical bone name.
    pub bone_channels: HashMap<String, BoneChannel>,
    /// Expression animation channels, keyed by expression name.
    pub expression_channels: HashMap<String, ExpressionChannel>,
    /// Look-at animation channel (at most one per clip).
    pub look_at_channel: Option<LookAtChannel>,
}

/// Semantic mapping from the `VRMC_vrm_animation` extension.
/// Maps humanoid bone names, expression names, and look-at to
/// glTF node indices.
#[derive(Clone, Debug, Default)]
pub struct VrmaProperties {
    /// Humanoid bone name → glTF node index.
    pub humanoid_bones: HashMap<String, usize>,
    /// Expression name → glTF node index.
    pub expressions: HashMap<String, usize>,
    /// Look-at glTF node index.
    pub look_at_node: Option<usize>,
    /// Look-at offset from head bone.
    pub look_at_offset: [f32; 3],
}

/// A loaded VRMA asset: semantic properties + animation clips.
#[derive(Clone, Debug)]
pub struct VrmaAsset {
    /// Semantic mapping (bone/expression/lookAt → node index).
    pub properties: VrmaProperties,
    /// Animation clips (usually one; the spec says the first
    /// is loaded by default).
    pub clips: Vec<VrmaClip>,
    /// Per-node rest transforms from the glTF node hierarchy.
    /// Index = glTF node index, value = local transform.
    pub node_rest_rotations: Vec<Quat>,
    /// Per-node rest positions. Index = glTF node index.
    pub node_rest_positions: Vec<Vec3>,
    /// Per-node world rest rotations (computed from hierarchy).
    pub node_world_rest_rotations: Vec<Quat>,
    /// Per-node world rest positions (computed from hierarchy).
    pub node_world_rest_positions: Vec<Vec3>,
    /// Per-node parent index (-1 for root).
    pub node_parents: Vec<i32>,
}

/// A single frame of sampled VRMA data at a specific time.
/// The consumer (desktop runtime) applies these to the VRM model.
#[derive(Clone, Debug, Default)]
pub struct VrmaFrame {
    /// Bone rotation deltas, keyed by canonical bone name.
    /// These are **retargeted** dest-local rotations ready to apply.
    pub bone_rotations: HashMap<String, Quat>,
    /// Dest **local** hips translation (absolute glTF local, not a world delta).
    pub hips_translation: Option<Vec3>,
    /// Expression weights, keyed by expression name. Clamped to `[0, 1]`.
    pub expression_weights: HashMap<String, f32>,
    /// Look-at yaw/pitch in degrees (if the clip has a look-at channel).
    pub look_at_yaw_pitch: Option<(f32, f32)>,
}

/// Repeat mode for playback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RepeatMode {
    /// Play once and stop.
    Once,
    /// Loop forever.
    #[default]
    Loop,
}

/// Per-frame playback state for a VRMA clip.
///
/// The player is deliberately separate from the clip data so
/// the same clip can be played by multiple players (e.g. for
/// crossfade transitions).
#[derive(Clone, Debug)]
pub struct VrmaPlayer {
    /// Current playback time in seconds.
    pub time: f32,
    /// Playback speed multiplier (1.0 = normal).
    pub speed: f32,
    /// Whether playback is active.
    pub playing: bool,
    pub repeat: RepeatMode,
}

impl Default for VrmaPlayer {
    fn default() -> Self {
        Self {
            time: 0.0,
            speed: 1.0,
            playing: false,
            repeat: RepeatMode::Loop,
        }
    }
}

impl VrmaPlayer {
    /// Start or resume playback.
    pub const fn play(&mut self) {
        self.playing = true;
    }

    /// Pause playback (preserves current time).
    pub const fn pause(&mut self) {
        self.playing = false;
    }

    /// Stop playback and reset to the beginning.
    pub const fn stop(&mut self) {
        self.playing = false;
        self.time = 0.0;
    }

    /// Seek to a specific time in seconds.
    pub const fn seek(&mut self, time: f32) {
        self.time = time.max(0.0);
    }

    /// Advance the playback clock by `dt` seconds.
    ///
    /// `duration` is the clip's total duration. The method
    /// handles looping and one-shot termination.
    pub fn advance(&mut self, dt: f32, duration: f32) {
        if !self.playing || duration <= 0.0 {
            return;
        }
        self.time = dt.mul_add(self.speed, self.time);
        match self.repeat {
            RepeatMode::Loop => {
                self.time %= duration;
                if self.time < 0.0 {
                    self.time += duration;
                }
            }
            RepeatMode::Once => {
                if self.time >= duration {
                    self.time = duration;
                    self.playing = false;
                }
                if self.time < 0.0 {
                    self.time = 0.0;
                    self.playing = false;
                }
            }
        }
    }
}

fn sample_scalar(sampler: &Sampler<f32>, t: f32) -> f32 {
    sample_keyframes(sampler, t, |a, b, alpha| a + (b - a) * alpha)
}

fn sample_vec3(sampler: &Sampler<Vec3>, t: f32) -> Vec3 {
    sample_keyframes(sampler, t, |a, b, alpha| a.lerp(*b, alpha))
}

fn sample_quat(sampler: &Sampler<Quat>, t: f32) -> Quat {
    match sampler.interpolation {
        Interpolation::Step => sample_step(&sampler.times, &sampler.values, t),
        Interpolation::Linear => {
            let idx = find_keyframe_index(&sampler.times, t);
            if idx + 1 >= sampler.times.len() {
                return *sampler.values.last().unwrap_or(&Quat::IDENTITY);
            }
            let t0 = sampler.times[idx];
            let t1 = sampler.times[idx + 1];
            let alpha = if (t1 - t0).abs() < 1e-9 {
                0.0
            } else {
                ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
            };
            sampler.values[idx].slerp(sampler.values[idx + 1], alpha)
        }
        Interpolation::CubicSpline => sample_cubic_spline_quat(sampler, t),
    }
}

fn sample_step<T: Clone + Default>(times: &[f32], values: &[T], t: f32) -> T {
    if values.is_empty() {
        return T::default();
    }
    if times.is_empty() {
        return values[0].clone();
    }
    if t <= times[0] {
        return values[0].clone();
    }
    for (i, &time) in times.iter().enumerate().skip(1) {
        if t < time {
            return values[i - 1].clone();
        }
    }
    values[values.len() - 1].clone()
}

fn find_keyframe_index(times: &[f32], t: f32) -> usize {
    if times.is_empty() {
        return 0;
    }
    if t <= times[0] {
        return 0;
    }
    for (i, &time) in times.iter().enumerate().skip(1) {
        if t < time {
            return i - 1;
        }
    }
    times.len().saturating_sub(2)
}

fn sample_keyframes<
    T: Clone + Default + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
>(
    sampler: &Sampler<T>,
    t: f32,
    lerp: impl Fn(&T, &T, f32) -> T,
) -> T {
    if sampler.values.is_empty() {
        return T::default();
    }
    if sampler.times.is_empty() {
        return sampler.values[0].clone();
    }
    match sampler.interpolation {
        Interpolation::Step => sample_step(&sampler.times, &sampler.values, t),
        Interpolation::Linear => {
            let idx = find_keyframe_index(&sampler.times, t);
            if idx + 1 >= sampler.times.len() {
                return sampler.values[sampler.values.len() - 1].clone();
            }
            let t0 = sampler.times[idx];
            let t1 = sampler.times[idx + 1];
            let alpha = if (t1 - t0).abs() < 1e-9 {
                0.0
            } else {
                ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
            };
            lerp(&sampler.values[idx], &sampler.values[idx + 1], alpha)
        }
        Interpolation::CubicSpline => {
            sample_cubic_spline(&sampler.times, &sampler.values, t, &lerp)
        }
    }
}

fn sample_cubic_spline<
    T: Clone + Default + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
>(
    times: &[f32],
    values: &[T],
    t: f32,
    _lerp: &impl Fn(&T, &T, f32) -> T,
) -> T {
    if values.len() < 3 {
        return values.first().cloned().unwrap_or_default();
    }
    if times.is_empty() {
        return values[1].clone();
    }
    let idx = find_keyframe_index(times, t);
    if idx + 1 >= times.len() {
        return values[values.len() - 2].clone();
    }
    let t0 = times[idx];
    let t1 = times[idx + 1];
    let dt = (t1 - t0).max(1e-9);
    let s = ((t - t0) / dt).clamp(0.0, 1.0);
    let s2 = s * s;
    let s3 = s2 * s;
    let h00 = 2.0f32.mul_add(s3, -3.0 * s2) + 1.0;
    let h10 = (s3 - 2.0f32.mul_add(s2, -s)) * dt;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = (s3 - s2) * dt;
    let base = idx * 3;
    let v0 = values[base + 1].clone();
    let m0 = values[base + 2].clone();
    let v1 = values[base + 4].clone();
    let m1 = values[base + 3].clone();
    v0 * h00 + m0 * h10 + v1 * h01 + m1 * h11
}

fn sample_cubic_spline_quat(sampler: &Sampler<Quat>, t: f32) -> Quat {
    let times = &sampler.times;
    let values = &sampler.values;
    if times.is_empty() {
        return Quat::IDENTITY;
    }
    let idx = find_keyframe_index(times, t);
    if idx + 1 >= times.len() {
        return values[values.len() - 2];
    }
    let t0 = times[idx];
    let t1 = times[idx + 1];
    let dt = (t1 - t0).max(1e-9);
    let s = ((t - t0) / dt).clamp(0.0, 1.0);
    let s2 = s * s;
    let s3 = s2 * s;
    let h00 = 3.0f32.mul_add(-s2, 2.0 * s3) + 1.0;
    let h10 = (2.0f32.mul_add(-s2, s3) + s) * dt;
    let h01 = 3.0f32.mul_add(s2, -2.0 * s3);
    let h11 = (s3 - s2) * dt;
    let base = idx * 3;
    let v0 = values[base + 1];
    let m0 = values[base];
    let v1 = values[base + 4];
    let m1 = values[base + 3];
    let q = v0 * h00 + m0 * h10 + v1 * h01 + m1 * h11;
    let norm = q.length();
    if norm < 1e-9 { v0 } else { q / norm }
}

/// Retarget a rotation from source skeleton to destination skeleton
/// using the VRM spec's NormalizedLocalRotation formula.
///
/// ```text
/// NLR = W_src * L_src^-1 * src_pose * W_src^-1
/// dst_pose = L_dst * W_dst^-1 * NLR * W_dst
/// ```
///
/// Not currently called by `ene-desktop` (VRoid-style models sharing a common T-pose
/// convention use the source local rotation directly); hidden from the crate's "Supported
/// API" docs (see `docs/reference/api/ene-vrm.md`) pending a consumer, but kept `pub` for other hosts
/// that need cross-skeleton retargeting.
#[doc(hidden)]
pub fn retarget_rotation(
    src_pose: Quat,
    src_rest_local: Quat,
    src_rest_global: Quat,
    dst_rest_local: Quat,
    dst_rest_global: Quat,
) -> Quat {
    let nlr = src_rest_global * src_rest_local.inverse() * src_pose * src_rest_global.inverse();
    dst_rest_local * dst_rest_global.inverse() * nlr * dst_rest_global
}

/// Retarget hips translation using height-based scaling.
///
/// ```text
/// delta = src_pose - src_rest_local
/// scale = dst_hips_height / src_hips_height
/// result = dst_rest_local + delta * scale
/// ```
///
/// See [`retarget_rotation`]'s note — not currently called by `ene-desktop`, hidden from the
/// "Supported API" docs but kept `pub` for other hosts.
#[doc(hidden)]
pub fn retarget_hips_translation(
    src_pose: Vec3,
    src_rest_local: Vec3,
    src_rest_global_y: f32,
    dst_rest_local: Vec3,
    dst_rest_global_y: f32,
) -> Vec3 {
    let delta = src_pose - src_rest_local;
    let scale = if src_rest_global_y.abs() < 1e-6 {
        1.0
    } else {
        dst_rest_global_y / src_rest_global_y
    };
    dst_rest_local + delta * scale
}

/// Decompose a quaternion into yaw/pitch using Extrinsic ZXY order.
///
/// The VRMA spec says: Y rotation = yaw, X rotation = pitch.
/// Yaw positive = model looks left, pitch positive = looks down.
///
/// Used internally by [`evaluate_clip`] to populate [`VrmaFrame::look_at_yaw_pitch`]; hidden
/// from the "Supported API" docs since `ene-desktop` consumes the resulting `VrmaFrame` field
/// rather than calling this directly.
#[doc(hidden)]
pub fn quat_to_yaw_pitch(q: Quat) -> (f32, f32) {
    let (yaw, pitch, _roll) = q.to_euler(glam::EulerRot::ZXY);
    (yaw.to_degrees(), pitch.to_degrees())
}

/// Evaluate a VRMA clip at time `t` and produce a [`VrmaFrame`].
///
/// The frame contains raw (un-retargeted) bone rotations and
/// expression weights. The consumer is responsible for retargeting
/// bone rotations onto the target VRM model's skeleton using
/// [`retarget_rotation`] and [`retarget_hips_translation`].
pub fn evaluate_clip(clip: &VrmaClip, t: f32) -> VrmaFrame {
    let mut frame = VrmaFrame::default();

    for (name, ch) in &clip.bone_channels {
        let rot = sample_quat(&ch.rotation, t);
        frame.bone_rotations.insert(name.clone(), rot);
        if name == "hips"
            && let Some(ref trans) = ch.translation
        {
            frame.hips_translation = Some(sample_vec3(trans, t));
        }
    }

    for (name, ch) in &clip.expression_channels {
        let w = sample_scalar(&ch.weight, t).clamp(0.0, 1.0);
        frame.expression_weights.insert(name.clone(), w);
    }

    if let Some(ref ch) = clip.look_at_channel {
        let rot = sample_quat(&ch.rotation, t);
        let (yaw, pitch) = quat_to_yaw_pitch(rot);
        frame.look_at_yaw_pitch = Some((yaw, pitch));
    }

    frame
}

/// Map a VRMA source bone name onto a destination humanoid bone.
///
/// VRM 1.0 models expose `*thumbmetacarpal` / `*thumbproximal` /
/// `*thumbdistal`. Legacy VRM 0.x models use `*thumbproximal` /
/// `*thumbintermediate` / `*thumbdistal` instead. VRMA clips always
/// follow the 1.0 naming; when the destination lacks a metacarpal
/// segment, fold metacarpal motion onto proximal and proximal motion
/// onto intermediate.
fn resolve_vrma_dest_bone(vrma_bone: &str, dest: &HumanoidBoneRegistry) -> Option<VrmBone> {
    let vrm0_left_thumb = dest.by_name("leftthumbmetacarpal").is_none()
        && dest.by_name("leftthumbintermediate").is_some();
    let vrm0_right_thumb = dest.by_name("rightthumbmetacarpal").is_none()
        && dest.by_name("rightthumbintermediate").is_some();
    let mapped = match vrma_bone {
        "leftthumbmetacarpal" if vrm0_left_thumb => "leftthumbproximal",
        "leftthumbproximal" if vrm0_left_thumb => "leftthumbintermediate",
        "rightthumbmetacarpal" if vrm0_right_thumb => "rightthumbproximal",
        "rightthumbproximal" if vrm0_right_thumb => "rightthumbintermediate",
        other => other,
    };
    let bone = canonicalize_bone_name(mapped)?;
    if dest.lookup(&bone).is_some() {
        Some(bone)
    } else {
        None
    }
}

/// Destination rest pose slices used by [`evaluate_retargeted`].
#[derive(Clone, Copy, Debug)]
pub struct DestRestPose<'a> {
    pub local_rotations: &'a [Quat],
    pub world_rotations: &'a [Quat],
    pub local_positions: &'a [Vec3],
    pub world_positions: &'a [Vec3],
}

/// Evaluate a VRMA clip and retarget onto a destination humanoid rest pose.
///
/// Rotations use the VRM `NormalizedLocalRotation` formula. Hips translation
/// uses `dst_rest_local + (src_pose - src_rest_local) * (dst_global_y / src_global_y)`.
#[must_use]
pub fn evaluate_retargeted(
    asset: &VrmaAsset,
    clip: &VrmaClip,
    t: f32,
    dest_humanoid: &HumanoidBoneRegistry,
    dest: DestRestPose<'_>,
) -> VrmaFrame {
    let raw = evaluate_clip(clip, t);
    let mut frame = VrmaFrame {
        bone_rotations: HashMap::new(),
        hips_translation: None,
        expression_weights: raw.expression_weights,
        look_at_yaw_pitch: raw.look_at_yaw_pitch,
    };
    for (name, src_pose) in &raw.bone_rotations {
        let Some(dest_bone) = resolve_vrma_dest_bone(name, dest_humanoid) else {
            continue;
        };
        let Some(dst_entry) = dest_humanoid.lookup(&dest_bone) else {
            continue;
        };
        let Some(&src_node) = asset.properties.humanoid_bones.get(name) else {
            frame.bone_rotations.insert(dest_bone.0.clone(), *src_pose);
            continue;
        };
        let Some(src_rest_local) = asset.node_rest_rotations.get(src_node).copied() else {
            frame.bone_rotations.insert(dest_bone.0.clone(), *src_pose);
            continue;
        };
        let Some(src_rest_world) = asset.node_world_rest_rotations.get(src_node).copied() else {
            frame.bone_rotations.insert(dest_bone.0.clone(), *src_pose);
            continue;
        };
        let Some(dst_rest_local) = dest.local_rotations.get(dst_entry.node).copied() else {
            continue;
        };
        let Some(dst_rest_world) = dest.world_rotations.get(dst_entry.node).copied() else {
            continue;
        };
        let dst_pose = retarget_rotation(
            *src_pose,
            src_rest_local,
            src_rest_world,
            dst_rest_local,
            dst_rest_world,
        );
        frame.bone_rotations.insert(dest_bone.0.clone(), dst_pose);
    }
    if let Some(src_pose) = raw.hips_translation {
        frame.hips_translation = retarget_hips_to_dest(asset, dest_humanoid, dest, src_pose);
    }
    frame
}

fn retarget_hips_to_dest(
    asset: &VrmaAsset,
    dest_humanoid: &HumanoidBoneRegistry,
    dest: DestRestPose<'_>,
    src_pose: Vec3,
) -> Option<Vec3> {
    let hips = dest_humanoid.hips()?;
    let dst_rest_local = *dest.local_positions.get(hips.node)?;
    let dst_rest_world_y = dest.world_positions.get(hips.node)?.y;
    let Some(&src_node) = asset.properties.humanoid_bones.get("hips") else {
        return Some(dst_rest_local);
    };
    let src_rest_local = *asset.node_rest_positions.get(src_node)?;
    let src_rest_world_y = asset.node_world_rest_positions.get(src_node)?.y;
    Some(retarget_hips_translation(
        src_pose,
        src_rest_local,
        src_rest_world_y,
        dst_rest_local,
        dst_rest_world_y,
    ))
}

/// Load a `.vrma` file from disk and parse it into a [`VrmaAsset`].
///
/// A VRMA file is a standard glTF/GLB binary with the
/// `VRMC_vrm_animation` extension. The function reads the
/// semantic mapping (bone/expression/lookAt → node index) and
/// all animation clips.
pub fn load_vrma(path: impl AsRef<Path>) -> VrmResult<VrmaAsset> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| VrmError::Io {
        path: path.display().to_string(),
        source,
    })?;

    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| VrmError::Gltf(e.to_string()))?;

    if !gltf
        .document
        .extensions_used()
        .any(|e| e == "VRMC_vrm_animation")
    {
        return Err(VrmError::Gltf(
            "missing VRMC_vrm_animation extension".into(),
        ));
    }

    let properties = parse_vrma_properties(&gltf);
    let (
        node_rest_rotations,
        node_rest_positions,
        node_world_rest_rotations,
        node_world_rest_positions,
        node_parents,
    ) = compute_node_rest_transforms(&gltf);

    let mut clips = Vec::new();
    for (i, anim) in gltf.document.animations().enumerate() {
        let clip = parse_animation(&gltf, &anim, &properties, i);
        clips.push(clip);
    }

    Ok(VrmaAsset {
        properties,
        clips,
        node_rest_rotations,
        node_rest_positions,
        node_world_rest_rotations,
        node_world_rest_positions,
        node_parents,
    })
}

fn parse_vrma_properties(gltf: &gltf::Gltf) -> VrmaProperties {
    let mut props = VrmaProperties::default();

    let Some(ext) = gltf.document.extensions() else {
        return props;
    };
    let Some(vrma_ext) = ext.get("VRMC_vrm_animation") else {
        return props;
    };

    if let Some(humanoid) = vrma_ext.get("humanoid").and_then(|v| v.as_object())
        && let Some(human_bones) = humanoid.get("humanBones").and_then(|v| v.as_object())
    {
        for (bone_name, entry) in human_bones {
            if let Some(bone) = canonicalize_bone_name(bone_name)
                && let Some(node_idx) = entry
                    .get("node")
                    .and_then(serde_json::Value::as_u64)
                    .map(|v| v as usize)
            {
                props.humanoid_bones.insert(bone.0, node_idx);
            }
        }
    }

    if let Some(expressions) = vrma_ext.get("expressions").and_then(|v| v.as_object()) {
        for category in ["preset", "custom"] {
            if let Some(cat) = expressions.get(category).and_then(|v| v.as_object()) {
                for (name, entry) in cat {
                    if let Some(node_idx) = entry
                        .get("node")
                        .and_then(serde_json::Value::as_u64)
                        .map(|v| v as usize)
                    {
                        props.expressions.insert(name.clone(), node_idx);
                    }
                }
            }
        }
    }

    if let Some(look_at) = vrma_ext.get("lookAt").and_then(|v| v.as_object()) {
        if let Some(node_idx) = look_at
            .get("node")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize)
        {
            props.look_at_node = Some(node_idx);
        }
        if let Some(offset) = look_at.get("offsetFromHeadBone").and_then(|v| v.as_array())
            && offset.len() == 3
        {
            props.look_at_offset = [
                offset[0].as_f64().unwrap_or(0.0) as f32,
                offset[1].as_f64().unwrap_or(0.06) as f32,
                offset[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
    }

    props
}

/// Return type of [`compute_node_rest_transforms`]: local rotations,
/// local positions, world rotations, world positions, and parent indices.
type NodeRestTransforms = (Vec<Quat>, Vec<Vec3>, Vec<Quat>, Vec<Vec3>, Vec<i32>);

fn compute_node_rest_transforms(gltf: &gltf::Gltf) -> NodeRestTransforms {
    let node_count = gltf.document.nodes().count();
    let mut local_rotations = vec![Quat::IDENTITY; node_count];
    let mut local_positions = vec![Vec3::ZERO; node_count];
    let mut parents = vec![-1i32; node_count];

    for (i, node) in gltf.document.nodes().enumerate() {
        let (t, r, _s) = node.transform().decomposed();
        local_positions[i] = Vec3::from(t);
        local_rotations[i] = Quat::from_array(r);
    }

    for node in gltf.document.nodes() {
        let idx = node.index();
        for child in node.children() {
            parents[child.index()] = idx as i32;
        }
    }

    let mut world_rotations = vec![Quat::IDENTITY; node_count];
    let mut world_positions = vec![Vec3::ZERO; node_count];

    for i in 0..node_count {
        // Walk parent chain root → node. Stop only when parent < 0
        // (do not special-case node index 0 — it may not be the root).
        let mut chain = Vec::new();
        let mut cur = i as i32;
        loop {
            chain.push(cur as usize);
            let p = parents[cur as usize];
            if p < 0 {
                break;
            }
            cur = p;
        }
        chain.reverse();

        // Standard glTF hierarchy:
        //   world_pos' = world_pos + world_rot * local_pos
        //   world_rot' = world_rot * local_rot
        let mut wr = Quat::IDENTITY;
        let mut wp = Vec3::ZERO;
        for &n in &chain {
            wp += wr * local_positions[n];
            wr *= local_rotations[n];
        }
        world_rotations[i] = wr;
        world_positions[i] = wp;
    }

    (
        local_rotations,
        local_positions,
        world_rotations,
        world_positions,
        parents,
    )
}

fn parse_animation(
    gltf: &gltf::Gltf,
    anim: &gltf::Animation,
    props: &VrmaProperties,
    index: usize,
) -> VrmaClip {
    let name = anim
        .name()
        .map_or_else(|| format!("clip_{index}"), String::from);

    let mut bone_channels: HashMap<String, BoneChannel> = HashMap::new();
    let mut expression_channels: HashMap<String, ExpressionChannel> = HashMap::new();
    let mut look_at_channel: Option<LookAtChannel> = None;

    let reverse_bones: HashMap<usize, String> = props
        .humanoid_bones
        .iter()
        .map(|(k, &v)| (v, k.clone()))
        .collect();
    let reverse_exprs: HashMap<usize, String> = props
        .expressions
        .iter()
        .map(|(k, &v)| (v, k.clone()))
        .collect();

    for channel in anim.channels() {
        let target_node = channel.target().node().index();
        let target_prop = channel.target().property();

        let reader = channel.reader(|_| gltf.blob.as_deref());
        let interpolation = Interpolation::from_gltf(channel.sampler().interpolation());

        let input_times: Vec<f32> = match reader.read_inputs() {
            Some(iter) => iter.collect(),
            None => continue,
        };

        if input_times.is_empty() {
            continue;
        }

        if let Some(bone_name) = reverse_bones.get(&target_node) {
            match target_prop {
                gltf::animation::Property::Rotation => {
                    let quats: Vec<Quat> = match reader.read_outputs() {
                        Some(gltf::animation::util::ReadOutputs::Rotations(rots)) => rots
                            .into_f32()
                            .map(|[x, y, z, w]| Quat::from_xyzw(x, y, z, w))
                            .collect(),
                        _ => continue,
                    };
                    let ch =
                        bone_channels
                            .entry(bone_name.clone())
                            .or_insert_with(|| BoneChannel {
                                bone_name: bone_name.clone(),
                                rotation: Sampler {
                                    times: input_times.clone(),
                                    values: Vec::new(),
                                    interpolation,
                                },
                                translation: None,
                            });
                    ch.rotation = Sampler {
                        times: input_times,
                        values: quats,
                        interpolation,
                    };
                }
                gltf::animation::Property::Translation => {
                    let vecs: Vec<Vec3> = match reader.read_outputs() {
                        Some(gltf::animation::util::ReadOutputs::Translations(trans)) => {
                            trans.map(|[x, y, z]| Vec3::new(x, y, z)).collect()
                        }
                        _ => continue,
                    };
                    let ch =
                        bone_channels
                            .entry(bone_name.clone())
                            .or_insert_with(|| BoneChannel {
                                bone_name: bone_name.clone(),
                                rotation: Sampler {
                                    times: Vec::new(),
                                    values: Vec::new(),
                                    interpolation: Interpolation::Linear,
                                },
                                translation: None,
                            });
                    ch.translation = Some(Sampler {
                        times: input_times,
                        values: vecs,
                        interpolation,
                    });
                }
                _ => {}
            }
        } else if let Some(expr_name) = reverse_exprs.get(&target_node) {
            if target_prop == gltf::animation::Property::Translation {
                let scalars: Vec<f32> = match reader.read_outputs() {
                    Some(gltf::animation::util::ReadOutputs::Translations(trans)) => {
                        trans.map(|[x, _y, _z]| x).collect()
                    }
                    _ => continue,
                };
                expression_channels.insert(
                    expr_name.clone(),
                    ExpressionChannel {
                        name: expr_name.clone(),
                        weight: Sampler {
                            times: input_times,
                            values: scalars,
                            interpolation,
                        },
                    },
                );
            }
        } else if props.look_at_node == Some(target_node)
            && target_prop == gltf::animation::Property::Rotation
        {
            let quats: Vec<Quat> = match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Rotations(rots)) => rots
                    .into_f32()
                    .map(|[x, y, z, w]| Quat::from_xyzw(x, y, z, w))
                    .collect(),
                _ => continue,
            };
            look_at_channel = Some(LookAtChannel {
                rotation: Sampler {
                    times: input_times,
                    values: quats,
                    interpolation,
                },
                offset_from_head_bone: props.look_at_offset,
            });
        }
    }

    let mut duration = 0.0f32;
    for ch in bone_channels.values() {
        duration = duration.max(ch.rotation.duration());
        if let Some(ref t) = ch.translation {
            duration = duration.max(t.duration());
        }
    }
    for ch in expression_channels.values() {
        duration = duration.max(ch.weight.duration());
    }
    if let Some(ref ch) = look_at_channel {
        duration = duration.max(ch.rotation.duration());
    }

    VrmaClip {
        name,
        duration,
        bone_channels,
        expression_channels,
        look_at_channel,
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;

    #[test]
    fn vrma_player_default_state() {
        let p = VrmaPlayer::default();
        assert_eq!(p.time, 0.0);
        assert_eq!(p.speed, 1.0);
        assert!(!p.playing);
        assert_eq!(p.repeat, RepeatMode::Loop);
    }

    #[test]
    fn player_advance_loop_wraps() {
        let mut p = VrmaPlayer::default();
        p.play();
        p.advance(1.5, 1.0);
        assert!((p.time - 0.5).abs() < 1e-6);
        assert!(p.playing);
    }

    #[test]
    fn player_advance_once_stops_at_end() {
        let mut p = VrmaPlayer {
            repeat: RepeatMode::Once,
            ..Default::default()
        };
        p.play();
        p.advance(2.0, 1.0);
        assert!((p.time - 1.0).abs() < 1e-6);
        assert!(!p.playing);
    }

    #[test]
    fn player_stop_resets_time() {
        let mut p = VrmaPlayer::default();
        p.play();
        p.advance(0.5, 1.0);
        p.stop();
        assert_eq!(p.time, 0.0);
        assert!(!p.playing);
    }

    #[test]
    fn player_seek_clamps_negative() {
        let mut p = VrmaPlayer::default();
        p.seek(-1.0);
        assert_eq!(p.time, 0.0);
    }

    #[test]
    fn player_speed_multiplier() {
        let mut p = VrmaPlayer {
            speed: 2.0,
            ..Default::default()
        };
        p.play();
        p.advance(0.5, 10.0);
        assert!((p.time - 1.0).abs() < 1e-6);
    }

    #[test]
    fn player_paused_does_not_advance() {
        let mut p = VrmaPlayer::default();
        p.advance(1.0, 10.0);
        assert_eq!(p.time, 0.0);
    }

    #[test]
    fn sample_scalar_step() {
        let s = Sampler {
            times: vec![0.0, 1.0, 2.0],
            values: vec![0.0, 0.5, 1.0],
            interpolation: Interpolation::Step,
        };
        assert!((sample_scalar(&s, 0.0) - 0.0).abs() < 1e-6);
        assert!((sample_scalar(&s, 0.5) - 0.0).abs() < 1e-6);
        assert!((sample_scalar(&s, 1.0) - 0.5).abs() < 1e-6);
        assert!((sample_scalar(&s, 1.5) - 0.5).abs() < 1e-6);
        assert!((sample_scalar(&s, 2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sample_scalar_linear() {
        let s = Sampler {
            times: vec![0.0, 1.0],
            values: vec![0.0, 1.0],
            interpolation: Interpolation::Linear,
        };
        assert!((sample_scalar(&s, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sample_vec3_linear() {
        let s = Sampler {
            times: vec![0.0, 1.0],
            values: vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0)],
            interpolation: Interpolation::Linear,
        };
        let v = sample_vec3(&s, 0.5);
        assert!((v - Vec3::new(0.5, 1.0, 1.5)).length() < 1e-6);
    }

    #[test]
    fn sample_quat_linear_slerp() {
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let s = Sampler {
            times: vec![0.0, 1.0],
            values: vec![q0, q1],
            interpolation: Interpolation::Linear,
        };
        let q = sample_quat(&s, 0.5);
        let expected = q0.slerp(q1, 0.5);
        assert!((q.x - expected.x).abs() < 1e-5);
        assert!((q.y - expected.y).abs() < 1e-5);
        assert!((q.z - expected.z).abs() < 1e-5);
        assert!((q.w - expected.w).abs() < 1e-5);
    }

    #[test]
    fn sample_quat_step() {
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let s = Sampler {
            times: vec![0.0, 1.0],
            values: vec![q0, q1],
            interpolation: Interpolation::Step,
        };
        let q = sample_quat(&s, 0.5);
        assert!((q.z - q0.z).abs() < 1e-5);
    }

    #[test]
    fn retarget_rotation_identity_is_identity() {
        let src_pose = Quat::from_rotation_y(0.5);
        let result = retarget_rotation(
            src_pose,
            Quat::IDENTITY,
            Quat::IDENTITY,
            Quat::IDENTITY,
            Quat::IDENTITY,
        );
        assert!((result.x - src_pose.x).abs() < 1e-5);
        assert!((result.y - src_pose.y).abs() < 1e-5);
        assert!((result.z - src_pose.z).abs() < 1e-5);
        assert!((result.w - src_pose.w).abs() < 1e-5);
    }

    #[test]
    fn retarget_hips_translation_no_scale_when_equal_height() {
        let result = retarget_hips_translation(
            Vec3::new(0.0, 1.1, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            1.0,
            Vec3::new(0.0, 1.0, 0.0),
            1.0,
        );
        assert!((result.y - 1.1).abs() < 1e-6);
    }

    #[test]
    fn retarget_hips_translation_scales_by_height_ratio() {
        let result = retarget_hips_translation(
            Vec3::new(0.0, 1.2, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            1.0,
            Vec3::new(0.0, 1.0, 0.0),
            2.0,
        );
        let delta = result.y - 1.0;
        assert!((delta - 0.4).abs() < 1e-6);
    }

    #[test]
    fn quat_to_yaw_pitch_identity() {
        let (yaw, pitch) = quat_to_yaw_pitch(Quat::IDENTITY);
        assert!(yaw.abs() < 1e-5);
        assert!(pitch.abs() < 1e-5);
    }

    #[test]
    fn evaluate_clip_empty() {
        let clip = VrmaClip {
            name: "empty".into(),
            duration: 0.0,
            bone_channels: HashMap::new(),
            expression_channels: HashMap::new(),
            look_at_channel: None,
        };
        let frame = evaluate_clip(&clip, 0.0);
        assert!(frame.bone_rotations.is_empty());
        assert!(frame.expression_weights.is_empty());
        assert!(frame.look_at_yaw_pitch.is_none());
    }

    #[test]
    fn evaluate_clip_samples_bone_rotation() {
        let clip = VrmaClip {
            name: "test".into(),
            duration: 1.0,
            bone_channels: {
                let mut m = HashMap::new();
                m.insert(
                    "hips".into(),
                    BoneChannel {
                        bone_name: "hips".into(),
                        rotation: Sampler {
                            times: vec![0.0, 1.0],
                            values: vec![
                                Quat::IDENTITY,
                                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                            ],
                            interpolation: Interpolation::Linear,
                        },
                        translation: None,
                    },
                );
                m
            },
            expression_channels: HashMap::new(),
            look_at_channel: None,
        };
        let frame = evaluate_clip(&clip, 0.5);
        assert!(frame.bone_rotations.contains_key("hips"));
    }

    #[test]
    fn evaluate_clip_clamps_expression_weight() {
        let clip = VrmaClip {
            name: "test".into(),
            duration: 1.0,
            bone_channels: HashMap::new(),
            expression_channels: {
                let mut m = HashMap::new();
                m.insert(
                    "happy".into(),
                    ExpressionChannel {
                        name: "happy".into(),
                        weight: Sampler {
                            times: vec![0.0, 1.0],
                            values: vec![-0.5, 1.5],
                            interpolation: Interpolation::Linear,
                        },
                    },
                );
                m
            },
            look_at_channel: None,
        };
        let frame = evaluate_clip(&clip, 0.5);
        let w = frame.expression_weights["happy"];
        assert!((0.0..=1.0).contains(&w), "weight {w} not clamped to [0,1]");
    }

    #[test]
    fn evaluate_clip_look_at() {
        let clip = VrmaClip {
            name: "test".into(),
            duration: 1.0,
            bone_channels: HashMap::new(),
            expression_channels: HashMap::new(),
            look_at_channel: Some(LookAtChannel {
                rotation: Sampler {
                    times: vec![0.0],
                    values: vec![Quat::from_rotation_y(0.5)],
                    interpolation: Interpolation::Linear,
                },
                offset_from_head_bone: [0.0, 0.06, 0.0],
            }),
        };
        let frame = evaluate_clip(&clip, 0.0);
        assert!(frame.look_at_yaw_pitch.is_some());
    }

    #[test]
    fn sampler_duration_from_last_timestamp() {
        let s = Sampler {
            times: vec![0.0, 0.5, 1.5],
            values: vec![0.0f32, 0.5, 1.0],
            interpolation: Interpolation::Linear,
        };
        assert!((s.duration() - 1.5).abs() < 1e-6);
    }

    #[test]
    fn sampler_duration_empty_is_zero() {
        let s: Sampler<f32> = Sampler {
            times: vec![],
            values: vec![],
            interpolation: Interpolation::Linear,
        };
        assert_eq!(s.duration(), 0.0);
    }

    #[test]
    fn sampler_keyframe_count() {
        let s = Sampler {
            times: vec![0.0, 1.0, 2.0],
            values: vec![0.0f32, 0.5, 1.0],
            interpolation: Interpolation::Linear,
        };
        assert_eq!(s.keyframe_count(), 3);
    }

    #[test]
    fn evaluate_retargeted_hips_are_dest_local_not_world_additive() {
        use crate::humanoid::{BoneRestTransform, HumanoidBoneEntry, VrmBone};
        let rest = Vec3::new(0.0, 0.9, 0.0);
        let sample = Vec3::new(0.0, 0.95, 0.0);
        let mut bones = HashMap::new();
        bones.insert("hips".to_owned(), 0);
        let asset = VrmaAsset {
            properties: VrmaProperties {
                humanoid_bones: bones,
                ..VrmaProperties::default()
            },
            clips: Vec::new(),
            node_rest_rotations: vec![Quat::IDENTITY],
            node_rest_positions: vec![rest],
            node_world_rest_rotations: vec![Quat::IDENTITY],
            node_world_rest_positions: vec![rest],
            node_parents: vec![-1],
        };
        let clip = VrmaClip {
            name: "idle".into(),
            duration: 1.0,
            bone_channels: {
                let mut m = HashMap::new();
                m.insert(
                    "hips".into(),
                    BoneChannel {
                        bone_name: "hips".into(),
                        rotation: Sampler {
                            times: vec![0.0],
                            values: vec![Quat::IDENTITY],
                            interpolation: Interpolation::Step,
                        },
                        translation: Some(Sampler {
                            times: vec![0.0],
                            values: vec![sample],
                            interpolation: Interpolation::Step,
                        }),
                    },
                );
                m
            },
            expression_channels: HashMap::new(),
            look_at_channel: None,
        };
        let mut dest_humanoid = HumanoidBoneRegistry::new();
        dest_humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform {
                    translation: rest,
                    rotation: Quat::IDENTITY,
                },
            },
        );
        let local_rot = [Quat::IDENTITY];
        let world_rot = [Quat::IDENTITY];
        let local_pos = [rest];
        let world_pos = [rest];
        let frame = evaluate_retargeted(
            &asset,
            &clip,
            0.0,
            &dest_humanoid,
            DestRestPose {
                local_rotations: &local_rot,
                world_rotations: &world_rot,
                local_positions: &local_pos,
                world_positions: &world_pos,
            },
        );
        let hips = frame.hips_translation.expect("hips");
        assert!(
            (hips - sample).length() < 1e-5,
            "expected dest local {sample:?}, got {hips:?} (must not be ~1.85)"
        );
        assert!(hips.y < 1.2, "hips y {hips:?} looks world-additive");
    }

    #[test]
    fn evaluate_retargeted_y_only_does_not_shift_x() {
        use crate::humanoid::{BoneRestTransform, HumanoidBoneEntry, VrmBone};
        let rest = Vec3::new(0.1, 0.9, 0.0);
        let sample = Vec3::new(0.1, 0.95, 0.0);
        let mut bones = HashMap::new();
        bones.insert("hips".to_owned(), 0);
        let asset = VrmaAsset {
            properties: VrmaProperties {
                humanoid_bones: bones,
                ..VrmaProperties::default()
            },
            clips: Vec::new(),
            node_rest_rotations: vec![Quat::IDENTITY],
            node_rest_positions: vec![rest],
            node_world_rest_rotations: vec![Quat::IDENTITY],
            node_world_rest_positions: vec![rest],
            node_parents: vec![-1],
        };
        let clip = VrmaClip {
            name: "idle".into(),
            duration: 1.0,
            bone_channels: {
                let mut m = HashMap::new();
                m.insert(
                    "hips".into(),
                    BoneChannel {
                        bone_name: "hips".into(),
                        rotation: Sampler {
                            times: vec![0.0],
                            values: vec![Quat::IDENTITY],
                            interpolation: Interpolation::Step,
                        },
                        translation: Some(Sampler {
                            times: vec![0.0],
                            values: vec![sample],
                            interpolation: Interpolation::Step,
                        }),
                    },
                );
                m
            },
            expression_channels: HashMap::new(),
            look_at_channel: None,
        };
        let mut dest_humanoid = HumanoidBoneRegistry::new();
        dest_humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform {
                    translation: rest,
                    rotation: Quat::IDENTITY,
                },
            },
        );
        let local_rot = [Quat::IDENTITY];
        let world_rot = [Quat::IDENTITY];
        let local_pos = [rest];
        let world_pos = [rest];
        let frame = evaluate_retargeted(
            &asset,
            &clip,
            0.0,
            &dest_humanoid,
            DestRestPose {
                local_rotations: &local_rot,
                world_rotations: &world_rot,
                local_positions: &local_pos,
                world_positions: &world_pos,
            },
        );
        let hips = frame.hips_translation.expect("hips");
        assert!((hips.x - 0.1).abs() < 1e-5, "x shifted: {hips:?}");
        assert!((hips.y - 0.95).abs() < 1e-5, "y wrong: {hips:?}");
    }

    #[test]
    fn resolve_vrma_dest_bone_maps_thumb_chain_for_vrm0() {
        use crate::humanoid::{BoneRestTransform, HumanoidBoneEntry, VrmBone};
        let mut vrm0 = HumanoidBoneRegistry::new();
        for (name, node) in [
            ("leftthumbproximal", 70),
            ("leftthumbintermediate", 71),
            ("leftthumbdistal", 72),
        ] {
            vrm0.insert(
                VrmBone(name.into()),
                HumanoidBoneEntry {
                    node,
                    joint: None,
                    rest: BoneRestTransform::default(),
                },
            );
        }
        assert_eq!(
            resolve_vrma_dest_bone("leftthumbmetacarpal", &vrm0)
                .unwrap()
                .0,
            "leftthumbproximal"
        );
        assert_eq!(
            resolve_vrma_dest_bone("leftthumbproximal", &vrm0)
                .unwrap()
                .0,
            "leftthumbintermediate"
        );
        assert_eq!(
            resolve_vrma_dest_bone("leftthumbdistal", &vrm0).unwrap().0,
            "leftthumbdistal"
        );

        let mut vrm1 = HumanoidBoneRegistry::new();
        for (name, node) in [
            ("leftthumbmetacarpal", 10),
            ("leftthumbproximal", 11),
            ("leftthumbdistal", 12),
        ] {
            vrm1.insert(
                VrmBone(name.into()),
                HumanoidBoneEntry {
                    node,
                    joint: None,
                    rest: BoneRestTransform::default(),
                },
            );
        }
        assert_eq!(
            resolve_vrma_dest_bone("leftthumbmetacarpal", &vrm1)
                .unwrap()
                .0,
            "leftthumbmetacarpal"
        );
        assert_eq!(
            resolve_vrma_dest_bone("leftthumbproximal", &vrm1)
                .unwrap()
                .0,
            "leftthumbproximal"
        );
    }

    #[test]
    fn evaluate_retargeted_emits_vrm0_thumb_bone_names() {
        use crate::humanoid::{BoneRestTransform, HumanoidBoneEntry, VrmBone};
        let mut dest_humanoid = HumanoidBoneRegistry::new();
        for (name, node) in [
            ("leftthumbproximal", 70),
            ("leftthumbintermediate", 71),
            ("leftthumbdistal", 72),
        ] {
            dest_humanoid.insert(
                VrmBone(name.into()),
                HumanoidBoneEntry {
                    node,
                    joint: Some(node),
                    rest: BoneRestTransform::default(),
                },
            );
        }
        let mut src_bones = HashMap::new();
        for (name, node) in [
            ("leftthumbmetacarpal", 0),
            ("leftthumbproximal", 1),
            ("leftthumbdistal", 2),
        ] {
            src_bones.insert(name.to_owned(), node);
        }
        let asset = VrmaAsset {
            properties: VrmaProperties {
                humanoid_bones: src_bones,
                ..VrmaProperties::default()
            },
            clips: Vec::new(),
            node_rest_rotations: vec![Quat::IDENTITY; 3],
            node_rest_positions: vec![Vec3::ZERO; 3],
            node_world_rest_rotations: vec![Quat::IDENTITY; 3],
            node_world_rest_positions: vec![Vec3::ZERO; 3],
            node_parents: vec![-1, -1, -1],
        };
        let mut bone_channels = HashMap::new();
        for name in [
            "leftthumbmetacarpal",
            "leftthumbproximal",
            "leftthumbdistal",
        ] {
            bone_channels.insert(
                name.to_owned(),
                BoneChannel {
                    bone_name: name.to_owned(),
                    rotation: Sampler {
                        times: vec![0.0],
                        values: vec![Quat::from_rotation_y(0.5)],
                        interpolation: Interpolation::Step,
                    },
                    translation: None,
                },
            );
        }
        let clip = VrmaClip {
            name: "thumb".into(),
            duration: 1.0,
            bone_channels,
            expression_channels: HashMap::new(),
            look_at_channel: None,
        };
        let local_rot = vec![Quat::IDENTITY; 73];
        let world_rot = vec![Quat::IDENTITY; 73];
        let local_pos = vec![Vec3::ZERO; 73];
        let world_pos = vec![Vec3::ZERO; 73];
        let frame = evaluate_retargeted(
            &asset,
            &clip,
            0.0,
            &dest_humanoid,
            DestRestPose {
                local_rotations: &local_rot,
                world_rotations: &world_rot,
                local_positions: &local_pos,
                world_positions: &world_pos,
            },
        );
        assert!(frame.bone_rotations.contains_key("leftthumbproximal"));
        assert!(frame.bone_rotations.contains_key("leftthumbintermediate"));
        assert!(frame.bone_rotations.contains_key("leftthumbdistal"));
        assert!(!frame.bone_rotations.contains_key("leftthumbmetacarpal"));
    }
}
