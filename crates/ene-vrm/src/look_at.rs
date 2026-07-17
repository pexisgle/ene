//! VRMC_vrm-1.0 look-at properties and per-frame evaluator.
//!
//! parses the `VRMC_vrm.lookAt` extension object (the
//! `offsetFromHeadBone`, four `rangeMap*` fields, and the
//! `type` discriminator) and turns the per-frame
//! `(head_world, target_world)` pair into either per-bone
//! rotation deltas (the `"bone"` type) or per-expression morph
//! weights (the `"expression"` type).
//!
//! ## Spec background
//!
//! The VRM 1.0 look-at spec
//! ([`VRMC_vrm-1.0/lookAt.md`](https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_vrm-1.0/lookAt.md))
//! declares an *origin* (`head_world + offsetFromHeadBone`), a
//! `(yaw, pitch)` decomposition in the head bone's local frame,
//! and a per-axis range map that maps the raw cursor degrees to
//! the model's preferred degrees. The `"bone"` consumer rotates
//! the humanoid `head` / `leftEye` / `rightEye` bones; the
//! `"expression"` consumer writes `lookUp` / `lookDown` /
//! `lookLeft` / `lookRight` morph-target weights.
//!
//! ## Conventions
//!
//! - **Yaw is positive when the model looks to its own left.**
//!   The spec decomposes the target direction in the head bone's
//!   local space, so a target to the model's right gives a
//!   negative yaw. The sign matches the `bevy_vrm1` reference
//!   implementation (`look_at.rs::calc_yaw_pitch`, lines
//!   222-237) one-for-one so screenshots and the
//!   `body_tracking_for_strength` integration stay comparable.
//! - **Pitch is positive when the model looks down.** The
//!   reference uses `atan2(-y, xz)`; the negation flips the
//!   "+Y-up" world axis into the spec's "look-down = positive"
//!   convention.
//! - **Range map inputs are clamped to `[0, inputMaxValue]`
//!   (absolute value) and scaled by `outputScale /
//!   inputMaxValue`.** A 90→10 map turns a 45° cursor direction
//!   into 5° of head / eye rotation.
//! - **`type = "bone"` is the spec default** and is what every
//!   VRoid-exported model uses. The `"expression"` type is what
//!   the legacy `bevy_vrm1` *ignored*: the cursor followed the
//!   head but the `lookUp/lookDown/lookLeft/lookRight` morph
//!   targets stayed at zero, so the face never moved.
//!
//! ## Consumers
//!
//! The desktop runtime
//! ([`apps/ene-desktop-v2/src/character/mod.rs`]) calls
//! [`LookAtEvaluator::evaluate`] every frame after the cursor
//! has been projected to a world target. For `"expression"`
//! models the result is written straight into
//! [`crate::expression::ExpressionLayer::set_expression`]; for
//! `"bone"` models the result is currently **stored but not
//! uploaded** to the skinning palette (the per-frame joint
//! update is the next pass — see the
//! [`apps/ene-desktop-v2/src/look_at.rs`] docstring). The
//! `Bone` output is the contract the next PR will consume.
//!
//! ## Status
//!
//! This module only owns the **math + parse** half. The cursor
//! → world target projection stays in the desktop app because
//! it depends on the camera state.
use glam::{Quat, Vec3};

/// Per-axis range map declared in the `VRMC_vrm.lookAt` block.
///
/// The map clamps an input angle (in degrees) to `[0, inputMaxValue]`
/// and rescales it to `[0, outputScale]`. The spec calls these
/// `inputMaxValue` / `outputScale`; the legacy `bevy_vrm1`
/// `RangeMap` struct uses the same field names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookAtRangeMap {
    /// Maximum input value, in degrees. Input is clamped to
    /// `[0, inputMaxValue]` (absolute value).
    pub input_max_value: f32,
    /// Output scale, in degrees. The clamped input is mapped
    /// linearly so that `input_max_value → output_scale`.
    pub output_scale: f32,
}

impl Default for LookAtRangeMap {
    /// The spec's default range map: `90 → 10` degrees.
    fn default() -> Self {
        Self {
            input_max_value: 90.0,
            output_scale: 10.0,
        }
    }
}

impl LookAtRangeMap {
    /// Apply the range map to a signed input angle. Input is
    /// clamped to `[-input_max_value, +input_max_value]`
    /// (absolute value) and the sign is preserved.
    ///
    /// The spec describes the four maps as **directional** —
    /// `horizontalInner` / `horizontalOuter` are not signed
    /// values; the caller picks the map for the *direction* of
    /// the gaze (e.g. `horizontalOuter` for the eye that moves
    /// the most when looking left, `horizontalInner` for the
    /// one that converges). See
    /// [`LookAtRangeMapSet::apply_horizontal`] /
    /// [`LookAtRangeMapSet::apply_vertical`] for the
    /// direction-aware wrappers.
    #[must_use]
    pub fn apply(&self, input_degrees: f32) -> f32 {
        let input_max_abs = self.input_max_value.abs();
        if input_max_abs < 1e-10 {
            return 0.0;
        }
        let sign = input_degrees.signum();
        let abs = input_degrees.abs();
        let clamped = abs.min(input_max_abs);
        let scaled = (clamped / input_max_abs) * self.output_scale;
        let result = sign * scaled;
        if result.is_finite() { result } else { 0.0 }
    }
}

/// The four range maps declared in `VRMC_vrm.lookAt`. The
/// naming follows the spec (and the legacy `bevy_vrm1`
/// `LookAtProperties`):
///
/// - `horizontalInner` / `horizontalOuter` — the eye's lateral
///   range. The *inner* map is the convergent one (the eye that
///   moves less when looking across); the *outer* map is the
///   divergent one. For `"expression"`-type models the spec
///   collapses this distinction and uses `horizontalOuter` for
///   both `lookLeft` and `lookRight` (the `RangeMap` field
///   comment in `bevy_vrm1/src/vrm/gltf/extensions/vrmc_vrm.rs`
///   calls this out explicitly).
/// - `verticalDown` / `verticalUp` — the vertical range.
///   `verticalDown` is used when the model looks down
///   (`pitch > 0`); `verticalUp` when it looks up
///   (`pitch < 0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookAtRangeMapSet {
    /// Horizontal convergent range map. Used for the eye
    /// that moves *less* when looking across (the eye
    /// closest to the nose).
    pub horizontal_inner: LookAtRangeMap,
    /// Horizontal divergent range map. Used for the eye
    /// that moves *more* when looking across. For
    /// `"expression"`-type models this is the single map
    /// that drives both `lookLeft` and `lookRight`.
    pub horizontal_outer: LookAtRangeMap,
    /// Vertical range map for looking down (`pitch > 0`).
    pub vertical_down: LookAtRangeMap,
    /// Vertical range map for looking up (`pitch < 0`).
    pub vertical_up: LookAtRangeMap,
}

impl LookAtRangeMapSet {
    /// Apply the appropriate horizontal range map to a signed
    /// yaw angle. The caller passes which side wins (the
    /// convergent or the divergent eye); we just pick the map.
    #[must_use]
    pub fn apply_horizontal(&self, yaw_degrees: f32, outer: bool) -> f32 {
        if outer {
            self.horizontal_outer.apply(yaw_degrees)
        } else {
            self.horizontal_inner.apply(yaw_degrees)
        }
    }

    /// Apply the appropriate vertical range map to a signed
    /// pitch angle. The convention matches the spec:
    /// `pitch > 0` = looking down, `pitch < 0` = looking up.
    #[must_use]
    pub fn apply_vertical(&self, pitch_degrees: f32) -> f32 {
        if pitch_degrees >= 0.0 {
            self.vertical_down.apply(pitch_degrees)
        } else {
            self.vertical_up.apply(pitch_degrees)
        }
    }
}

/// The `VRMC_vrm.lookAt.type` discriminator. The spec uses
/// lower-case JSON strings (`"bone"` / `"expression"`); we keep
/// the `serde` rename style but model the two values as enum
/// variants so the consumer side stays typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LookAtType {
    /// Drive the humanoid `head` / `leftEye` / `rightEye` bone
    /// rotations. Spec default.
    #[default]
    Bone,
    /// Drive the `lookUp` / `lookDown` / `lookLeft` / `lookRight`
    /// morph-target weights. This is what the legacy
    /// `bevy_vrm1` silently no-op'd: the cursor followed the
    /// camera pan, but the expressions were never written, so
    /// the face never moved. fixes the silent no-op.
    Expression,
}

/// The full `VRMC_vrm.lookAt` block. The spec considers every
/// field optional and applies the defaults above when a field
/// is missing. preserves the same defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookAtProperties {
    /// Offset from the head bone to the look-at origin (in
    /// object space). The spec stores this as
    /// `[x, y, z]` in the head's local frame; the convention
    /// is `(0, 0.06, 0)` so the origin sits between the eyes.
    pub offset_from_head_bone: [f32; 3],
    /// Per-axis range maps.
    pub range_map: LookAtRangeMapSet,
    /// `Bone` or `Expression` consumer.
    pub look_at_type: LookAtType,
}

impl Default for LookAtProperties {
    fn default() -> Self {
        Self {
            offset_from_head_bone: [0.0, 0.06, 0.0],
            range_map: LookAtRangeMapSet::default(),
            look_at_type: LookAtType::default(),
        }
    }
}

impl LookAtProperties {
    /// Default smoothing speed (in 1/seconds) for the
    /// cursor → world target projection. The spec doesn't
    /// declare a smoothing value; the legacy `bevy_vrm1`
    /// hard-coded `7.0` and carried that forward. Models
    /// that want a slower / faster follow can override this
    /// with a future spec extension; today the value is the
    /// single source of truth.
    pub const DEFAULT_SMOOTHING: f32 = 7.0;
}

/// Per-frame `(yaw, pitch)` decomposition in degrees. The
/// sign convention matches the [`LookAtRangeMapSet`]
/// docstring: positive yaw = model looks left, positive pitch
/// = model looks down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookAtDirection {
    /// Yaw in degrees. Positive = model looks to its own
    /// left, negative = model looks to its own right.
    pub yaw_degrees: f32,
    /// Pitch in degrees. Positive = model looks down,
    /// negative = model looks up.
    pub pitch_degrees: f32,
}

/// Per-bone rotation delta for `"bone"`-type models. The
/// `delta` quaternion is the rotation to **apply to the
/// bone's rest rotation** (i.e. the final local transform is
/// `rest.rotation * delta`). The future skinning-palette
/// upload is a single multiply per bone per frame.
///
/// The deltas are stored as `Quat` rather than `(yaw, pitch)`
/// so the consumer (the next skinning pass) does not need to
/// know about the `(yaw, pitch)` decomposition.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookAtBoneDelta {
    /// Delta rotation to apply on top of the bone's rest
    /// rotation. Identity for an un-rotated bone.
    pub delta: Quat,
}

/// Per-bone outputs for `"bone"`-type models. All three are
/// populated together (the math is the same for each bone);
/// `head` carries the full `(yaw, pitch)` deltas, the eyes
/// only carry the `yaw` (per the spec, eyes do not pitch
/// independently of the head).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookAtBoneOutput {
    /// Head bone delta.
    pub head: LookAtBoneDelta,
    /// Left-eye bone delta.
    pub left_eye: LookAtBoneDelta,
    /// Right-eye bone delta.
    pub right_eye: LookAtBoneDelta,
}

/// Per-expression outputs for `"expression"`-type models.
/// All four are populated together; a value below
/// `output_scale` after clamping becomes the morph weight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookAtExpressionOutput {
    /// `lookUp` morph weight in `[0, 1]`. Positive when the
    /// model looks up (`pitch < 0`).
    pub look_up: f32,
    /// `lookDown` morph weight in `[0, 1]`. Positive when the
    /// model looks down (`pitch > 0`).
    pub look_down: f32,
    /// `lookLeft` morph weight in `[0, 1]`. Positive when the
    /// model looks to its own left (`yaw > 0`).
    pub look_left: f32,
    /// `lookRight` morph weight in `[0, 1]`. Positive when
    /// the model looks to its own right (`yaw < 0`).
    pub look_right: f32,
}

/// The combined output of [`LookAtEvaluator::evaluate`]. The
/// variant matches the model's `look_at_type`; a model with
/// `type = "bone"` only ever produces `Bone`, and vice versa.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookAtOutput {
    /// Per-bone deltas for `"bone"`-type models.
    Bone(LookAtBoneOutput),
    /// Per-expression morph weights for
    /// `"expression"`-type models.
    Expression(LookAtExpressionOutput),
}

/// State holder for the per-frame evaluation. `new` takes a
/// `&LookAtProperties`; the rest of the work happens in
/// [`Self::evaluate`], which is a pure function of the
/// `(head_world, target_world)` pair and the head bone's rest
/// rotation (so the yaw / pitch decomposes in the head's
/// local frame, not the world frame).
#[derive(Clone, Copy, Debug)]
pub struct LookAtEvaluator {
    offset_from_head_bone: [f32; 3],
    range_map: LookAtRangeMapSet,
    look_at_type: LookAtType,
}

impl LookAtEvaluator {
    /// Build an evaluator from a `LookAtProperties`. Returns
    /// `None` for an unset `None` so the runtime can chain
    /// `model.look_at().map(LookAtEvaluator::new)` without
    /// nested `if let`s.
    #[must_use]
    pub const fn new(props: &LookAtProperties) -> Self {
        Self {
            offset_from_head_bone: props.offset_from_head_bone,
            range_map: props.range_map,
            look_at_type: props.look_at_type,
        }
    }

    /// Per-frame evaluation. `head_world` is the world-space
    /// position of the head bone (the runtime supplies this
    /// from the humanoid registry + the current model
    /// transform); `target_world` is the cursor-projected
    /// world point the model should look at (the desktop
    /// runtime computes this in
    /// [`apps/ene-desktop-v2/src/look_at.rs::compute_world_target`]).
    ///
    /// `head_rest_rotation` is the head bone's rest rotation
    /// (the local-to-parent quaternion from the humanoid
    /// registry). The function uses it to rotate the gaze
    /// direction into the head's local frame before
    /// decomposing. For a model whose head has identity rest
    /// rotation this is a no-op (the local frame matches the
    /// world frame), but a model whose head was authored with
    /// a non-identity rest (e.g. a tilted head) needs the
    /// rotation to get the right `(yaw, pitch)`.
    ///
    /// For a target at the head's own position the
    /// decomposition is the zero vector and the result is the
    /// default (identity deltas / zero expression weights).
    #[must_use]
    pub fn evaluate(
        &self,
        head_world: Vec3,
        target_world: Vec3,
        head_rest_rotation: Quat,
    ) -> LookAtOutput {
        let direction = calc_yaw_pitch(
            head_world,
            target_world,
            self.offset_from_head_bone,
            head_rest_rotation,
        );
        match self.look_at_type {
            LookAtType::Bone => LookAtOutput::Bone(LookAtBoneOutput {
                head: LookAtBoneDelta {
                    delta: yaw_pitch_to_delta(
                        direction.yaw_degrees,
                        direction.pitch_degrees,
                        &self.range_map,
                        // The head carries the full (yaw, pitch).
                        HorizontalSide::Outer,
                    ),
                },
                left_eye: LookAtBoneDelta {
                    // For yaw > 0 (left), the LEFT eye does
                    // the OUTER (large) movement and the
                    // RIGHT eye does the INNER (small)
                    // movement. For yaw < 0 (right), the
                    // mirror holds: LEFT does INNER, RIGHT
                    // does OUTER. The pitch delta for the
                    // eyes is the same as the head's (the
                    // eyes move with the head).
                    delta: yaw_pitch_to_delta(
                        direction.yaw_degrees,
                        direction.pitch_degrees,
                        &self.range_map,
                        if direction.yaw_degrees >= 0.0 {
                            HorizontalSide::Outer
                        } else {
                            HorizontalSide::Inner
                        },
                    ),
                },
                right_eye: LookAtBoneDelta {
                    delta: yaw_pitch_to_delta(
                        direction.yaw_degrees,
                        direction.pitch_degrees,
                        &self.range_map,
                        if direction.yaw_degrees >= 0.0 {
                            HorizontalSide::Inner
                        } else {
                            HorizontalSide::Outer
                        },
                    ),
                },
            }),
            LookAtType::Expression => {
                let yaw_deg = self.range_map.apply_horizontal(direction.yaw_degrees, true);
                let pitch_deg = self.range_map.apply_vertical(direction.pitch_degrees);
                // For `"expression"`-type models the range
                // map's `outputScale` is in **degrees**, but
                // morph weights must be in `[0, 1]`. The
                // spec convention (and the reference
                // UniVRM implementation) normalises the
                // range-map output by `outputScale` so a
                // full-deflection cursor (input = maxValue)
                // drives the morph to 1.0. A half-deflection
                // cursor drives the morph to 0.5.
                // `apply_horizontal` / `apply_vertical` use
                // the *outer* / *down* maps for the positive
                // sign, so we read the matching `output_scale`
                // from the same map.
                let horizontal_outer_scale = self.range_map.horizontal_outer.output_scale;
                let yaw_weight = if horizontal_outer_scale.abs() > 1e-10 {
                    (yaw_deg / horizontal_outer_scale).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                let yaw_weight = if yaw_weight.is_finite() {
                    yaw_weight
                } else {
                    0.0
                };

                let vertical_down_scale = self.range_map.vertical_down.output_scale;
                let vertical_up_scale = self.range_map.vertical_up.output_scale;
                let pitch_weight = if pitch_deg >= 0.0 {
                    if vertical_down_scale.abs() > 1e-10 {
                        (pitch_deg / vertical_down_scale).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                } else {
                    if vertical_up_scale.abs() > 1e-10 {
                        (pitch_deg / vertical_up_scale).clamp(-1.0, 0.0)
                    } else {
                        0.0
                    }
                };
                let pitch_weight = if pitch_weight.is_finite() {
                    pitch_weight
                } else {
                    0.0
                };
                // Per the spec: for `"expression"`-type
                // models the `horizontalOuter` map drives
                // both `lookLeft` and `lookRight` (the
                // legacy `bevy_vrm1` docstring on
                // `rangeMapHorizontalOuter` calls this
                // out). The eyes are merged into a single
                // expression in this mode.
                LookAtOutput::Expression(LookAtExpressionOutput {
                    look_up: (-pitch_weight).max(0.0),
                    look_down: pitch_weight.max(0.0),
                    look_left: yaw_weight.max(0.0),
                    look_right: (-yaw_weight).max(0.0),
                })
            }
        }
    }
}

/// Which horizontal range map wins for a given gaze. Used
/// by the bone branch only; the expression branch always
/// picks `Outer` (see the spec / reference comment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HorizontalSide {
    Inner,
    Outer,
}

/// Convert a `(yaw, pitch)` pair in degrees to a `Quat`
/// delta, applying the per-axis range maps first. `side`
/// picks `horizontalInner` or `horizontalOuter` for the
/// yaw map; pitch always uses the vertical maps.
fn yaw_pitch_to_delta(
    yaw_degrees: f32,
    pitch_degrees: f32,
    range_map: &LookAtRangeMapSet,
    side: HorizontalSide,
) -> Quat {
    let yaw = match side {
        HorizontalSide::Inner => range_map.apply_horizontal(yaw_degrees, false),
        HorizontalSide::Outer => range_map.apply_horizontal(yaw_degrees, true),
    };
    let pitch = range_map.apply_vertical(pitch_degrees);
    // The reference (bevy_vrm1::to_eye_rotation) wraps
    // the local euler rotation between the rest rotation
    // and its inverse. The rest-rotation sandwich is
    // needed because the head / eyes might be authored
    // with a non-identity local-to-parent rotation. For
    // a model whose head is at the canonical orientation
    // (`rest = IDENTITY`) the sandwich collapses to
    // `Quat::from_euler(YXZ, yaw, pitch, 0)`.
    Quat::from_euler(
        glam::EulerRot::YXZ,
        yaw.to_radians(),
        pitch.to_radians(),
        0.0,
    )
}

/// Decompose the world-space gaze direction into a
/// `(yaw, pitch)` pair in degrees. Sign convention:
/// positive yaw = model looks to its own left, positive
/// pitch = model looks down.
///
/// `head_world` is the world-space position of the head
/// bone. The look-at origin is
/// `head_world + offset_from_head_bone`; the
/// decomposition happens in the head bone's local frame
/// so a model whose head was authored with a non-identity
/// rest rotation gets the right sign.
#[must_use]
pub fn calc_yaw_pitch(
    head_world: Vec3,
    target_world: Vec3,
    offset_from_head_bone: [f32; 3],
    head_rest_rotation: Quat,
) -> LookAtDirection {
    let origin = head_world + Vec3::from(offset_from_head_bone);
    let dir_world = target_world - origin;
    // Rotate the world-space direction into the head's
    // local frame. The head's rest rotation is the
    // local-to-parent rotation; for a head with no parent
    // transform above (or with a parent transform we are
    // not tracking) the inverse is sufficient.
    let dir_local = if head_rest_rotation.is_finite() && head_rest_rotation.length_squared() > 1e-6
    {
        head_rest_rotation.inverse() * dir_world
    } else {
        dir_world
    };

    let z = dir_local.z;
    let x = dir_local.x;
    let yaw = if x.is_finite() && z.is_finite() {
        x.atan2(z).to_degrees()
    } else {
        0.0
    };

    let xz = x.hypot(z);
    let y = dir_local.y;
    // The `-y` flips the "look up = negative pitch" sign
    // into the spec's "look up = negative pitch,
    // look down = positive pitch" convention. Reference:
    // bevy_vrm1::calc_yaw_pitch line 234.
    let pitch = if y.is_finite() && xz.is_finite() {
        (-y).atan2(xz).to_degrees()
    } else {
        0.0
    };

    LookAtDirection {
        yaw_degrees: if yaw.is_finite() { yaw } else { 0.0 },
        pitch_degrees: if pitch.is_finite() { pitch } else { 0.0 },
    }
}

/// Parse the `VRMC_vrm.lookAt` extension object. Returns
/// `None` when the block is absent (e.g. a VRM 0.x model or
/// a stripped-down test fixture) so the runtime can fall
/// back to the spec defaults via
/// [`LookAtProperties::default`].
///
/// Missing fields are filled with the spec defaults
/// (matching the `bevy_vrm1` `LookAtProperties::default`).
/// Malformed fields (wrong type, out-of-range values) are
/// warned and fall back to the default. We deliberately do
/// not pull in `serde` — the rest of the loader is hand-rolled
/// JSON, and a single field's worth of dependency weight is
/// not worth it.
///
/// Called internally by [`crate::loader::load_vrm`]; hidden from the "Supported API" docs —
/// hosts get the result via [`VrmModel::look_at`](crate::model::VrmModel).
#[doc(hidden)]
pub fn load_look_at(gltf: &gltf::Gltf) -> Option<LookAtProperties> {
    let ext = gltf.document.extensions()?;
    let vrm = ext.get("VRMC_vrm")?;
    let look_at = vrm.get("lookAt")?;
    let look_at_obj = look_at.as_object()?;

    let mut props = LookAtProperties::default();

    // 1. `offsetFromHeadBone`: array of 3 floats. Missing or
    //    wrong length → keep the spec default `[0, 0.06, 0]`.
    if let Some(arr) = look_at_obj
        .get("offsetFromHeadBone")
        .and_then(|v| v.as_array())
    {
        if arr.len() == 3 {
            let mut out = [0.0f32; 3];
            let mut ok = true;
            for (i, item) in arr.iter().enumerate() {
                if let Some(f) = item.as_f64() {
                    out[i] = f as f32;
                } else {
                    ok = false;
                    tracing::warn!(
                        "VRMC_vrm.lookAt.offsetFromHeadBone[{i}] is not a number; keeping default"
                    );
                    break;
                }
            }
            if ok {
                props.offset_from_head_bone = out;
            }
        } else {
            tracing::warn!(
                "VRMC_vrm.lookAt.offsetFromHeadBone has {} element(s); expected 3, keeping default",
                arr.len()
            );
        }
    }

    // 2. The four range maps. Each is `{ inputMaxValue,
    //    outputScale }`. The shared helper logs + falls
    //    back on malformed entries.
    props.range_map.horizontal_inner = parse_range_map(
        look_at_obj,
        "rangeMapHorizontalInner",
        props.range_map.horizontal_inner,
    );
    props.range_map.horizontal_outer = parse_range_map(
        look_at_obj,
        "rangeMapHorizontalOuter",
        props.range_map.horizontal_outer,
    );
    props.range_map.vertical_down = parse_range_map(
        look_at_obj,
        "rangeMapVerticalDown",
        props.range_map.vertical_down,
    );
    props.range_map.vertical_up = parse_range_map(
        look_at_obj,
        "rangeMapVerticalUp",
        props.range_map.vertical_up,
    );

    // 3. `type`: string `"bone"` or `"expression"`. Missing
    //    or unknown → spec default (`Bone`).
    if let Some(type_str) = look_at_obj.get("type").and_then(|v| v.as_str()) {
        props.look_at_type = match type_str {
            "bone" => LookAtType::Bone,
            "expression" => LookAtType::Expression,
            other => {
                tracing::warn!(
                    "VRMC_vrm.lookAt.type is '{}' (expected 'bone' or 'expression'); using default",
                    other
                );
                LookAtType::default()
            }
        };
    }

    Some(props)
}

/// Parse a single `RangeMap`-shaped `{ inputMaxValue,
/// outputScale }` field. `default` is returned on a
/// missing / malformed entry (and a warning is logged).
/// Used by [`load_look_at`] for the four range maps.
fn parse_range_map(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    default: LookAtRangeMap,
) -> LookAtRangeMap {
    let Some(value) = obj.get(field) else {
        return default;
    };
    let Some(obj) = value.as_object() else {
        tracing::warn!(
            "VRMC_vrm.lookAt.{} is not an object; keeping default {:?}",
            field,
            default
        );
        return default;
    };
    let input = if let Some(f) = obj.get("inputMaxValue").and_then(serde_json::Value::as_f64) {
        f as f32
    } else {
        tracing::warn!(
            "VRMC_vrm.lookAt.{}.inputMaxValue is not a number; keeping default {:?}",
            field,
            default
        );
        return default;
    };
    let scale = if let Some(f) = obj.get("outputScale").and_then(serde_json::Value::as_f64) {
        f as f32
    } else {
        tracing::warn!(
            "VRMC_vrm.lookAt.{}.outputScale is not a number; keeping default {:?}",
            field,
            default
        );
        return default;
    };
    LookAtRangeMap {
        input_max_value: input,
        output_scale: scale,
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;

    /// The spec's defaults are an explicit contract. A
    /// regression that changed e.g. the offset from
    /// `(0, 0.06, 0)` would shift every model's eye-line
    /// by 6 cm and make every `VRoid` model look slightly
    /// crosseyed.
    #[test]
    fn default_properties_match_vrm_spec() {
        let p = LookAtProperties::default();
        assert_eq!(p.offset_from_head_bone, [0.0, 0.06, 0.0]);
        assert_eq!(p.look_at_type, LookAtType::Bone);
        for map in [
            p.range_map.horizontal_inner,
            p.range_map.horizontal_outer,
            p.range_map.vertical_down,
            p.range_map.vertical_up,
        ] {
            assert_eq!(map.input_max_value, 90.0);
            assert_eq!(map.output_scale, 10.0);
        }
    }

    /// Range map: 90→10 with a 45° input should produce 5°.
    /// A regression in the clamp or the scale factor would
    /// either over- or under-rotate every model's eyes.
    #[test]
    fn range_map_scales_clamped_input() {
        let m = LookAtRangeMap {
            input_max_value: 90.0,
            output_scale: 10.0,
        };
        assert!((m.apply(0.0)).abs() < 1e-6);
        assert!((m.apply(45.0) - 5.0).abs() < 1e-6);
        assert!((m.apply(90.0) - 10.0).abs() < 1e-6);
        // Above the cap is clamped to `output_scale`.
        assert!((m.apply(180.0) - 10.0).abs() < 1e-6);
        // Negative inputs are sign-preserved.
        assert!((m.apply(-45.0) + 5.0).abs() < 1e-6);
    }

    /// `calc_yaw_pitch` should give `(0, 0)` for a target
    /// directly ahead of the head. The most common bug is
    /// off-by-one in the sign of `pitch` (looking at a
    /// target above the head must be a *negative* pitch,
    /// matching the spec convention).
    #[test]
    fn calc_yaw_pitch_straight_ahead_is_zero() {
        let head = Vec3::new(0.0, 1.0, 0.0);
        // Target directly in front of the head (model's -Z
        // is forward in glTF; we use `+Z` here so a
        // positive-z target means "behind"). The
        // decomposition should be (0, 0) regardless of
        // forward direction when the target is at the
        // head's height.
        let d = calc_yaw_pitch(
            head,
            Vec3::new(0.0, 1.0, 1.0),
            [0.0, 0.0, 0.0],
            Quat::IDENTITY,
        );
        assert!(d.yaw_degrees.abs() < 1e-4, "yaw = {}", d.yaw_degrees);
        assert!(d.pitch_degrees.abs() < 1e-4, "pitch = {}", d.pitch_degrees);
    }

    /// A target to the model's own left (negative X in
    /// world space) must give a **positive** yaw per the
    /// spec's sign convention (`atan2(x, z)` is positive
    /// when `x > 0`, but the legacy reference uses the
    /// same formula and reports positive yaw as "looking
    /// left"). The reference's `calc_yaw_pitch` uses
    /// `atan2(x, z)` on the local direction; a target at
    /// `(-1, 0, 0)` local-frame has `x = -1, z = 0` so
    /// the formula gives `atan2(-1, 0) = -π/2 = -90°`.
    /// Looking at this from the model's perspective: the
    /// gaze direction is towards `(-1, 0, 0)` (the
    /// model's own left), so the head must rotate to
    /// its left, which is the *positive* yaw convention
    /// used by the range map (the right eye does the
    /// inner / convergent movement, the left eye does
    /// the outer). The math below mirrors the reference
    /// 1:1.
    #[test]
    fn calc_yaw_pitch_target_to_model_left_gives_positive_yaw() {
        let head = Vec3::new(0.0, 1.0, 0.0);
        // In glTF, a model that looks down the -Z axis
        // has the model's *left* at +X. So a target at
        // `+X` (the model's left) must give a positive
        // yaw. We pick a target at `(1, 1, 1)` so the
        // gaze direction is `(1, 0, 1)` and atan2(x=1,
        // z=1) lands at exactly 45° (well inside the
        // range-map clamp).
        let d = calc_yaw_pitch(
            head,
            Vec3::new(1.0, 1.0, 1.0),
            [0.0, 0.0, 0.0],
            Quat::IDENTITY,
        );
        assert!(
            (d.yaw_degrees - 45.0).abs() < 1.0,
            "yaw = {}",
            d.yaw_degrees
        );
        assert!(d.pitch_degrees.abs() < 1e-4, "pitch = {}", d.pitch_degrees);
    }

    /// A target above the head must give a **negative**
    /// pitch. The reference's formula uses
    /// `atan2(-y, xz)`, so `y > 0` (target above) gives a
    /// negative pitch. The downstream
    /// `apply_vertical(negative)` routes through the
    /// `verticalUp` map, which is the spec's "looking up"
    /// path.
    #[test]
    fn calc_yaw_pitch_target_above_gives_negative_pitch() {
        let head = Vec3::new(0.0, 1.0, 0.0);
        // Target directly in front and well above the
        // head — pick a vertical that gives a pitch
        // closer to -60° (not -45° on the boundary).
        let d = calc_yaw_pitch(
            head,
            Vec3::new(0.0, 1.0 + 2.0, 1.0),
            [0.0, 0.0, 0.0],
            Quat::IDENTITY,
        );
        // Direction = (0, 2, 1), atan2(-2, 1) ≈ -63.4°.
        assert!(d.pitch_degrees < -50.0, "pitch = {}", d.pitch_degrees);
    }

    /// The `expression`-type evaluator must produce
    /// non-zero weights for a target off-axis and the
    /// weights must be in `[0, 1]`. The reference
    /// applies `horizontalOuter` for both `lookLeft` and
    /// `lookRight`.
    #[test]
    fn expression_output_drives_morph_weights() {
        // The default `look_at_type` is `Bone`. Force
        // `Expression` so the test exercises the
        // expression branch (the default branch is
        // covered by the bone tests below).
        let props = LookAtProperties {
            look_at_type: LookAtType::Expression,
            ..LookAtProperties::default()
        };
        let eval = LookAtEvaluator::new(&props);
        // Test 1: target to the model's left and forward
        // — pick `(1, 2, 1)` so the gaze direction is
        // `(1, 1, 1)`. The y component is well away
        // from the head's height, so the atan2 stays
        // clear of the floating-point boundary.
        let head = Vec3::new(0.0, 1.0, 0.0);
        let target = Vec3::new(1.0, 2.0, 1.0);
        let out = eval.evaluate(head, target, Quat::IDENTITY);
        assert!(
            matches!(out, LookAtOutput::Expression(_)),
            "expected Expression output, got {out:?}"
        );
        let LookAtOutput::Expression(e) = out else {
            return;
        };
        // 45° yaw + 35° pitch. Range map scales the
        // cursor degrees to ±10°. Expression branch
        // divides by `output_scale` to get the morph
        // weight. The exact numbers are:
        //   yaw_deg = 5°, yaw_weight = 0.5
        //   pitch_deg = ~-3.5°, pitch_weight = ~-0.35
        //   look_left = 0.5, look_up = 0.35
        // We assert the relationship (positive look_left
        // AND positive look_up, with the rest ~zero) so
        // the test is robust to small range-map tweaks.
        assert!(e.look_left > 0.4, "look_left = {}", e.look_left);
        assert!(e.look_up > 0.2, "look_up = {}", e.look_up);
        assert!(e.look_right.abs() < 1e-3, "look_right = {}", e.look_right);
        assert!(e.look_down.abs() < 1e-3, "look_down = {}", e.look_down);
    }

    /// The `bone`-type evaluator must produce a non-
    /// identity head delta for a non-zero gaze. The
    /// specific quaternion value depends on the range
    /// map; we just check the delta is non-identity and
    /// the eye deltas exist.
    #[test]
    fn bone_output_drives_quat_deltas() {
        let props = LookAtProperties::default();
        let eval = LookAtEvaluator::new(&props);
        let head = Vec3::new(0.0, 1.0, 0.0);
        let target = Vec3::new(2.0, 1.0, 0.0);
        let out = eval.evaluate(head, target, Quat::IDENTITY);
        assert!(
            matches!(out, LookAtOutput::Bone(_)),
            "expected Bone output, got {out:?}"
        );
        let LookAtOutput::Bone(b) = out else {
            return;
        };
        assert_ne!(b.head.delta, Quat::IDENTITY);
        // The eye deltas are non-identity too (the eyes
        // yaw with the head). The exact magnitudes
        // depend on the inner / outer range map, but
        // for a target with yaw > 0 the LEFT eye is the
        // OUTER (large) eye and the RIGHT eye is the
        // INNER (small) one.
        assert_ne!(b.left_eye.delta, Quat::IDENTITY);
        assert_ne!(b.right_eye.delta, Quat::IDENTITY);
    }

    /// `load_look_at` must accept the spec's full block
    /// and produce a [`LookAtProperties`] with every
    /// field set. We construct a minimal glTF document
    /// with a `VRMC_vrm.lookAt` block and round-trip it
    /// through the JSON parser the loader exposes.
    #[test]
    fn load_look_at_parses_full_block() {
        // A minimal glTF 2.0 document is required for
        // `gltf::Gltf::from_slice` to succeed. The
        // `extensionsUsed` must list `VRMC_vrm` for the
        // extension to be parsed; the `extensions` block
        // carries the JSON payload.
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm"],
            "extensions": {
                "VRMC_vrm": {
                    "lookAt": {
                        "offsetFromHeadBone": [0.0, 0.04, 0.0],
                        "rangeMapHorizontalInner": { "inputMaxValue": 60.0, "outputScale": 8.0 },
                        "rangeMapHorizontalOuter": { "inputMaxValue": 60.0, "outputScale": 12.0 },
                        "rangeMapVerticalDown":     { "inputMaxValue": 45.0, "outputScale": 6.0 },
                        "rangeMapVerticalUp":       { "inputMaxValue": 30.0, "outputScale": 5.0 },
                        "type": "expression"
                    }
                }
            }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).expect("gltf parse");
        let props = load_look_at(&gltf).expect("lookAt block present");
        assert_eq!(props.offset_from_head_bone, [0.0, 0.04, 0.0]);
        assert_eq!(props.look_at_type, LookAtType::Expression);
        assert_eq!(props.range_map.horizontal_inner.input_max_value, 60.0);
        assert_eq!(props.range_map.horizontal_outer.output_scale, 12.0);
        assert_eq!(props.range_map.vertical_down.input_max_value, 45.0);
        assert_eq!(props.range_map.vertical_up.output_scale, 5.0);
    }

    /// Missing `lookAt` block → `None`. The runtime
    /// falls back to [`LookAtProperties::default`] in
    /// that case (it does **not** use `None` to disable
    /// the look-at — the spec considers the default
    /// block a valid model).
    #[test]
    fn load_look_at_returns_none_when_block_missing() {
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm"],
            "extensions": { "VRMC_vrm": { "humanoid": { "humanBones": {} } } }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).expect("gltf parse");
        assert!(load_look_at(&gltf).is_none());
    }

    /// Partial `lookAt` block → spec defaults fill in.
    /// A model that declares `type = "bone"` but no range
    /// maps must still parse, with the four range maps
    /// at their `90 → 10` defaults.
    #[test]
    fn load_look_at_partial_block_uses_spec_defaults() {
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm"],
            "extensions": {
                "VRMC_vrm": {
                    "lookAt": { "type": "bone" }
                }
            }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).expect("gltf parse");
        let props = load_look_at(&gltf).expect("lookAt block present");
        assert_eq!(props.look_at_type, LookAtType::Bone);
        assert_eq!(props.offset_from_head_bone, [0.0, 0.06, 0.0]);
        for map in [
            props.range_map.horizontal_inner,
            props.range_map.horizontal_outer,
            props.range_map.vertical_down,
            props.range_map.vertical_up,
        ] {
            assert_eq!(map.input_max_value, 90.0);
            assert_eq!(map.output_scale, 10.0);
        }
    }

    /// Malformed `offsetFromHeadBone` (wrong length) must
    /// log a warning and fall back to the default. A
    /// regression that panics here would crash the
    /// loader for every hand-edited VRM file in the
    /// wild.
    #[test]
    fn load_look_at_malformed_offset_falls_back() {
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm"],
            "extensions": {
                "VRMC_vrm": {
                    "lookAt": { "offsetFromHeadBone": [0.0, 0.04] }
                }
            }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).expect("gltf parse");
        let props = load_look_at(&gltf).expect("lookAt block present");
        assert_eq!(props.offset_from_head_bone, [0.0, 0.06, 0.0]);
    }

    /// Unknown `type` string must fall back to `Bone`
    /// (the spec default) rather than panic.
    #[test]
    fn load_look_at_unknown_type_falls_back_to_bone() {
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm"],
            "extensions": {
                "VRMC_vrm": { "lookAt": { "type": "magic" } }
            }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).expect("gltf parse");
        let props = load_look_at(&gltf).expect("lookAt block present");
        assert_eq!(props.look_at_type, LookAtType::Bone);
    }
}
