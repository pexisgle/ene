//! Per-bone collider specs and per-frame poses.
//!
//! Humanoid bones pick a shape that follows their role (limbs get
//! capsules along the parent→bone direction, the trunk gets a
//! vertical capsule, and the head / jaw / eyes / shoulders get
//! spheres), the dimensions come from the per-vertex skinning
//! weights of the actual mesh, and limbs are tracked using both
//! translation and rotation.
//!
//! `BoneShapeSpec` is the static data the collider builder needs
//! at construction time (local transform + Rapier shape, already
//! in world units). `BonePose` is the per-frame delta the runtime
//! pushes into the matching collider.
#![expect(
    dead_code,
    reason = "collider types are consumed by physics registration systems"
)]
use glam::{Mat4, Quat, Vec3};

use ene_vrm::{HumanoidBoneEntry, HumanoidBoneRegistry, MeshVertex, VrmModel};

/// Minimum bone radius (in world units, after `actual_scale`).
/// Below this the collider is too small to be useful and is
/// dropped — finger segments are the typical casualty. Kept
/// here (rather than `character/mod.rs`) so the `physics`
/// crate can also reference it when the spec list is empty.
pub const MIN_BONE_RADIUS: f32 = 0.025;

/// Vertex weight threshold for "this vertex belongs to this
/// bone". A vertex whose maximum skinning weight to the target
/// joint is below this is treated as belonging to a different
/// bone (the trunk's vertices are lightly weighted to nearby
/// bones too; without this filter the arm capsule would inflate
/// to include half the chest).
const VERTEX_WEIGHT_THRESHOLD: f32 = 0.25;

/// Shape category picked for a single humanoid bone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoneShape {
    /// Sphere of `radius` units, centred on the bone's rest
    /// position. Used for `head` / `jaw` / `eye*` / `shoulder*`
    /// / `*toes*` (round joints or small round extremities).
    Sphere { radius: f32 },
    /// Y-axis capsule of `half_height` + `radius` in the
    /// collider's local frame. The collider's `local_rotation`
    /// maps local +Y to whatever world direction the bone
    /// needs (vertical for the trunk, foot-aligned for the
    /// foot). Used for the spine / chest / hips / neck / foot.
    CapsuleY { half_height: f32, radius: f32 },
    /// Generic capsule whose local +Y is the bone's
    /// "toward-child" direction in the rest pose. Used for
    /// limbs (upperarm / lowerarm / upperleg / lowerleg /
    /// hand) where the local frame is the bone chain and the
    /// capsule needs to point along the chain direction
    /// rather than world up.
    Capsule { half_height: f32, radius: f32 },
}

impl BoneShape {
    /// Effective radius of the primitive (used by the
    /// `MIN_BONE_RADIUS` filter — fingers produce a small
    /// capsule that the filter drops).
    pub const fn radius(&self) -> f32 {
        match self {
            Self::Sphere { radius }
            | Self::CapsuleY { radius, .. }
            | Self::Capsule { radius, .. } => *radius,
        }
    }
}

/// Static data needed to build a Rapier collider for one
/// humanoid bone. Created once at model-load time by
/// [`compute_bone_specs`]. `local_position` and the shape
/// dimensions are already in world units (after
/// `actual_scale`), so [`crate::physics::PhysicsWorld`] can
/// drop them straight into Rapier.
#[derive(Clone, Copy, Debug)]
pub struct BoneShapeSpec {
    /// Node index of the bone in the glTF hierarchy.
    pub bone_node: usize,
    /// Local translation in the body's frame (the kinematic
    /// body sits at the world origin in the rest pose, so
    /// the collider's local position equals its world
    /// position offset by the body's translation).
    pub local_position: Vec3,
    /// Local rotation of the collider in the body's frame.
    /// Identity for spheres; the bone's rest world rotation
    /// for trunk / foot `CapsuleY`s; the limb's
    /// "toward-child" alignment for `Capsule`s.
    pub local_rotation: Quat,
    /// Picked shape + dimensions, in world units (already
    /// multiplied by `actual_scale`).
    pub shape: BoneShape,
    /// Static offset of the collider's center relative to the bone's position in rest pose,
    /// in world units (already multiplied by `actual_scale`).
    pub static_offset: Vec3,
    /// The bone's world rotation in rest pose.
    pub rest_rotation: Quat,
}

/// Per-frame delta for one bone. The runtime reads
/// [`crate::character::CharacterRenderer::current_bone_poses`]
/// every `about_to_wait` and pushes the entries into
/// `PhysicsWorld::update_character_bone_positions`.
#[derive(Clone, Copy, Debug)]
pub struct BonePose {
    /// Translation in the model's normalised frame (so
    /// `PhysicsWorld` can multiply by `actual_scale` to land
    /// in world units).
    pub translation: Vec3,
    /// Current world rotation of the bone (from
    /// `model.nodes.world_rotations[bone.node]`). For
    /// trunk / foot bones the collider's static rotation is
    /// the rest rotation, so the runtime bakes the delta
    /// into the body's translation; for limbs the per-frame
    /// rotation drives the capsule orientation directly.
    pub rotation: Quat,
}

/// Build the list of [`BoneShapeSpec`]s for every humanoid
/// bone on `model`. Bones whose computed radius falls below
/// [`MIN_BONE_RADIUS`] — typically the 24 finger segments and
/// the small toe bones — are dropped so the per-frame
/// `update_character_bone_positions` call stays cheap.
///
/// `actual_scale = auto_fit_scale × model_scale`. The returned
/// dimensions are already multiplied by this value so the
/// collider matches the rendered mesh even when the user has
/// not set `model_scale = 1.0`.
///
/// The order is deterministic (`BTreeMap` iteration on the
/// humanoid registry); [`crate::character::CharacterRenderer::current_bone_poses`]
/// returns poses in the same order so the runtime can
/// zip the two lists without re-keying.
pub fn classify_spring_chain(name: Option<&str>) -> &'static str {
    let Some(n) = name else {
        return "cloth";
    };
    let lower = n.to_lowercase();
    if lower.contains("hair")
        || lower.contains("fur")
        || lower.contains("tail")
        || lower.contains("ear")
    {
        "hair"
    } else {
        "cloth"
    }
}

pub fn compute_bone_specs(model: &VrmModel, actual_scale: f32) -> Vec<BoneShapeSpec> {
    let center = Vec3::from(model.center());
    let normalize_scale = model.normalize_scale();
    let rest_world = compute_rest_world_positions(model);
    let rest_world_rotations = compute_rest_world_rotations(model);

    let mut out = Vec::new();
    for (bone, entry) in model.humanoid.iter() {
        let Some(spec) = compute_bone_spec(
            model,
            entry,
            bone.as_str(),
            &rest_world,
            &rest_world_rotations,
            center,
            normalize_scale,
            actual_scale,
        ) else {
            continue;
        };
        out.push(spec);
    }

    if let Some(spring_bones) = &model.spring_bones {
        let scale = normalize_scale * actual_scale;
        for chain in &spring_bones.springs {
            for i in 0..chain.joints.len() {
                let joint = &chain.joints[i];
                if out.iter().any(|s| s.bone_node == joint.node) {
                    continue;
                }
                let parent_world = rest_world[joint.node];
                let local_position = (parent_world - center) * scale;
                let rest_rotation = rest_world_rotations[joint.node];
                let radius = joint.hit_radius.max(0.02) * scale;

                // Try to find if there is a child joint in the chain to build a segment-aligned capsule
                if i + 1 < chain.joints.len() {
                    let child_joint = &chain.joints[i + 1];
                    let child_world = rest_world[child_joint.node];
                    let axis = child_world - parent_world;
                    let axis_len = axis.length();
                    if axis_len > 1e-6 {
                        let axis_dir = axis / axis_len;
                        let half_height = 0.5 * axis_len * scale;
                        let rotation = Quat::from_rotation_arc(Vec3::Y, axis_dir);
                        let offset = 0.5 * axis * scale;

                        out.push(BoneShapeSpec {
                            bone_node: joint.node,
                            local_position,
                            local_rotation: rotation,
                            shape: BoneShape::Capsule {
                                half_height,
                                radius,
                            },
                            static_offset: offset,
                            rest_rotation,
                        });
                        continue;
                    }
                }

                // Fallback for terminal joints or degenerate segments: Sphere
                out.push(BoneShapeSpec {
                    bone_node: joint.node,
                    local_position,
                    local_rotation: Quat::IDENTITY,
                    shape: BoneShape::Sphere { radius },
                    static_offset: Vec3::ZERO,
                    rest_rotation,
                });
            }
        }
    }

    out
}

fn compute_bone_spec(
    model: &VrmModel,
    bone: &HumanoidBoneEntry,
    bone_name: &str,
    rest_world: &[Vec3],
    rest_world_rotations: &[Quat],
    center: Vec3,
    normalize_scale: f32,
    actual_scale: f32,
) -> Option<BoneShapeSpec> {
    // Gather vertices weighted to this bone. We need the
    // joint index to compare against `MeshVertex.joints`
    // (which carries Skeleton joint indices, not node
    // indices). `HumanoidBoneEntry::joint` is `None` for
    // bones outside the skin, in which case we drop the
    // collider — there is no skinning data to fit against.
    let joint_idx = bone.joint?;
    let inverse_bind = *model.skeleton.inverse_bind.get(joint_idx)?;
    let joint_world_rest = rest_joint_world(model, joint_idx);

    // Walk every primitive; collect each vertex's world
    // position weighted to this joint.
    let world_positions =
        collect_weighted_world_positions(model, joint_idx, &inverse_bind, &joint_world_rest);

    // `actual_scale` is in the normalised frame; `rest_world` is
    // in raw glTF space. The collider sits in the body frame,
    // which uses the normalised frame (matches the rendered
    // model after the body's translation is set to
    // `character_position`), so we apply both scales here.
    let bone_raw = rest_world[bone.node];
    let local_position = (bone_raw - center) * normalize_scale * actual_scale;

    // Fit the shape. `fit_limb_capsule` etc. operate in the
    // raw glTF space (where `rest_world` lives); we scale
    // the resulting dimensions by `normalize_scale *
    // actual_scale` so they match `local_position`'s frame.
    let (shape, local_rotation, static_offset) = fit_bone_shape(
        bone_name,
        bone.node,
        &world_positions,
        rest_world,
        rest_world_rotations,
        &model.humanoid,
        model,
        normalize_scale * actual_scale,
    );
    if shape.radius() < MIN_BONE_RADIUS {
        return None;
    }

    let rest_rotation = rest_world_rotations[bone.node];

    Some(BoneShapeSpec {
        bone_node: bone.node,
        local_position,
        local_rotation,
        shape,
        static_offset,
        rest_rotation,
    })
}

/// Walk every primitive and collect the world-space position
/// of each vertex whose dominant skinning weight targets
/// `target_joint`. Returns one entry per qualifying vertex
/// (the same vertex may be added more than once if its four
/// slots all point at the same joint — vanishingly rare in
/// well-formed VRM models).
fn collect_weighted_world_positions(
    model: &VrmModel,
    target_joint: usize,
    inverse_bind: &Mat4,
    joint_world_rest: &Mat4,
) -> Vec<Vec3> {
    let mut out = Vec::new();
    for primitive in model.meshes.iter().flat_map(|m| m.primitives.iter()) {
        collect_weighted_world_positions_into(
            &primitive.vertices,
            target_joint,
            inverse_bind,
            joint_world_rest,
            &mut out,
        );
    }
    out
}

/// Public version of the per-primitive collector: takes a
/// raw `&[MeshVertex]` slice so the unit tests can build a
/// fixture without constructing a full `VrmModel` /
/// `VrmPrimitive` pair (which needs a `wgpu::Device` for
/// the GPU buffers).
pub fn collect_weighted_world_positions_into(
    vertices: &[MeshVertex],
    target_joint: usize,
    inverse_bind: &Mat4,
    joint_world_rest: &Mat4,
    out: &mut Vec<Vec3>,
) {
    for v in vertices {
        // Take the maximum weight to `target_joint` across
        // the four slots (the same joint can appear in
        // multiple slots when the exporter duplicates a
        // slot for a "spillover" weight).
        let mut w = 0.0f32;
        for slot in 0..4 {
            if v.joints[slot] as usize == target_joint && v.weights[slot] > w {
                w = v.weights[slot];
            }
        }
        if w < VERTEX_WEIGHT_THRESHOLD {
            continue;
        }
        // Standard glTF skinning formula at rest:
        //   world_pos = joint_world_rest * inverse_bind * vertex
        let p = Vec3::from(v.position);
        let local = inverse_bind.transform_point3(p);
        let world = joint_world_rest.transform_point3(local);
        out.push(world);
    }
}

/// World-space rest transform of a Skeleton joint. Mirrors
/// what `update_skin_palette` would produce for that joint if
/// every node were at its rest pose. We do not have a direct
/// "joint world matrix" stored on the model (only per-node),
/// so we walk the parent chain to compose it: at rest,
/// `joint_world = Π (parent_local_rotation, parent_local_translation)`.
fn rest_joint_world(model: &VrmModel, joint_idx: usize) -> Mat4 {
    let Some(&node_idx) = model.skeleton.joint_to_node.get(joint_idx) else {
        return Mat4::IDENTITY;
    };
    let mut chain: Vec<usize> = Vec::new();
    let mut cur = node_idx as i32;
    while cur >= 0 {
        chain.push(cur as usize);
        cur = model.nodes.parents.get(cur as usize).copied().unwrap_or(-1);
    }
    chain.reverse();
    let mut acc = Mat4::IDENTITY;
    for node in chain {
        let rot = model.nodes.rest_local_rotations[node];
        let pos = model.nodes.rest_local_positions[node];
        acc = acc * Mat4::from_quat(rot) * Mat4::from_translation(pos);
    }
    acc
}

/// Walk the parent chain once and return the rest-pose world
/// position of every node. The character has ~30-50 humanoid
/// bones; calling this once at model-load time and re-using
/// the result for every `compute_bone_spec` call is far
/// cheaper than re-walking the chain per bone.
pub fn compute_rest_world_positions(model: &VrmModel) -> Vec<Vec3> {
    let mut world_positions = model.nodes.rest_local_positions.clone();
    let n = model.nodes.len();
    for i in 0..n {
        if let Some(&p) = model.nodes.parents.get(i)
            && p >= 0
        {
            let p = p as usize;
            if i > p {
                let rot_p = model.nodes.rest_local_rotations[p];
                let pos_p = world_positions[p];
                world_positions[i] = rot_p * model.nodes.rest_local_positions[i] + pos_p;
            }
        }
    }
    world_positions
}

pub fn compute_rest_world_rotations(model: &VrmModel) -> Vec<Quat> {
    let mut world_rotations = model.nodes.rest_local_rotations.clone();
    let n = model.nodes.len();
    for i in 0..n {
        if let Some(&p) = model.nodes.parents.get(i)
            && p >= 0
        {
            let p = p as usize;
            if i > p {
                let rot_p = world_rotations[p];
                world_rotations[i] = rot_p * model.nodes.rest_local_rotations[i];
            }
        }
    }
    world_rotations
}

/// Find the closest ancestor of `bone_node` that is also a
/// registered humanoid bone. Returns the ancestor's node
/// index, or `None` if the bone is the top of its humanoid
/// chain (e.g. `hips` → root).
#[cfg_attr(not(test), expect(dead_code, reason = "Test-only helper"))]
fn humanoid_parent_node(
    bone_node: usize,
    parents: &[i32],
    humanoid: &HumanoidBoneRegistry,
) -> Option<usize> {
    let mut cur = parents.get(bone_node).copied().unwrap_or(-1);
    while cur >= 0 {
        let n = cur as usize;
        if humanoid.iter().any(|(_, e)| e.node == n) {
            return Some(n);
        }
        cur = parents.get(n).copied().unwrap_or(-1);
    }
    None
}

/// Pick the shape category for `bone_name` and fit a tight
/// bounding primitive to the weighted vertex cloud. Returns
/// `(shape, collider_local_rotation, static_offset)` in world units. The
/// caller is responsible for the `MIN_BONE_RADIUS` filter.
fn fit_bone_shape(
    bone_name: &str,
    bone_node: usize,
    weighted_world: &[Vec3],
    rest_world: &[Vec3],
    _rest_world_rotations: &[Quat],
    humanoid: &HumanoidBoneRegistry,
    _model: &VrmModel,
    scale: f32,
) -> (BoneShape, Quat, Vec3) {
    let bone_world = rest_world[bone_node];
    match bone_name {
        "head" => {
            let mut max_y = bone_world.y;
            for &p in weighted_world {
                max_y = max_y.max(p.y);
            }
            let height_raw = (max_y - bone_world.y).clamp(0.16, 0.22);
            let radius = 0.52 * height_raw * scale;
            let offset_y = 0.5 * height_raw * scale;
            let static_offset = Vec3::new(0.0, offset_y, 0.0);

            (BoneShape::Sphere { radius }, Quat::IDENTITY, static_offset)
        }
        "jaw" | "lefteye" | "righteye" | "leftshoulder" | "rightshoulder" | "lefthand"
        | "righthand" | "lefttoes" | "righttoes" => {
            let default_radius = match bone_name {
                "lefteye" | "righteye" => 0.015,
                "leftshoulder" | "rightshoulder" => 0.055,
                "lefthand" | "righthand" => 0.045,
                "lefttoes" | "righttoes" => 0.04,
                _ => 0.05,
            };

            // Calculate max distance from bone_world to any vertex in the cloud
            let mut max_r = 0.0f32;
            for &p in weighted_world {
                max_r = max_r.max((p - bone_world).length());
            }

            // The radius must completely cover all these vertices, so we take max_r.
            // But we also bound it from below by the default_radius to handle empty/small clouds.
            let radius = max_r.max(default_radius) * scale;

            (BoneShape::Sphere { radius }, Quat::IDENTITY, Vec3::ZERO)
        }

        "hips" | "spine" | "chest" | "upperchest" | "neck" | "leftupperarm" | "leftlowerarm"
        | "rightupperarm" | "rightlowerarm" | "leftupperleg" | "leftlowerleg" | "rightupperleg"
        | "rightlowerleg" | "leftfoot" | "rightfoot" => {
            let default_radius = match bone_name {
                "hips" => 0.13,
                "spine" => 0.11,
                "chest" | "upperchest" => 0.12,
                "leftupperarm" | "rightupperarm" => 0.05,
                "leftlowerarm" | "rightlowerarm" => 0.04,
                "leftupperleg" | "rightupperleg" => 0.075,
                "leftlowerleg" | "rightlowerleg" => 0.06,
                "neck" | "leftfoot" | "rightfoot" => 0.045,
                _ => 0.05,
            };

            if let Some(child_node) = get_humanoid_child_node(bone_name, humanoid) {
                let child_world = rest_world[child_node];
                let axis = child_world - bone_world;
                let axis_len = axis.length();
                if axis_len > 1e-6 {
                    let axis_dir = axis / axis_len;
                    let half_height = 0.5 * axis_len * scale;
                    let rotation = Quat::from_rotation_arc(Vec3::Y, axis_dir);
                    let offset = 0.5 * axis * scale;

                    // Calculate max perpendicular distance from the segment to any vertex in the cloud
                    let mut max_perp = 0.0f32;
                    for &p in weighted_world {
                        let d = p - bone_world;
                        let t = (d.dot(axis_dir) / axis_len).clamp(0.0, 1.0);
                        let proj = bone_world + axis_dir * (t * axis_len);
                        let dist = (p - proj).length();
                        max_perp = max_perp.max(dist);
                    }

                    let radius = max_perp.max(default_radius) * scale;
                    return (
                        BoneShape::Capsule {
                            half_height,
                            radius,
                        },
                        rotation,
                        offset,
                    );
                }
            }

            // Fallback if child is missing or axis is degenerate: Sphere covering the cloud
            let mut max_r = 0.0f32;
            for &p in weighted_world {
                max_r = max_r.max((p - bone_world).length());
            }
            let radius = max_r.max(default_radius) * scale;
            (BoneShape::Sphere { radius }, Quat::IDENTITY, Vec3::ZERO)
        }

        _ => {
            let mut max_r = 0.0f32;
            for &p in weighted_world {
                max_r = max_r.max((p - bone_world).length());
            }
            let radius = max_r.max(0.01) * scale;
            (BoneShape::Sphere { radius }, Quat::IDENTITY, Vec3::ZERO)
        }
    }
}

/// Strip `left` / `right` from the canonical bone name so the
/// shape table only needs one match arm per central bone.
#[expect(
    dead_code,
    reason = "bone name helper retained for future collider tuning"
)]
fn strip_side_prefix(name: &str) -> &str {
    name.strip_prefix("left")
        .or_else(|| name.strip_prefix("right"))
        .unwrap_or(name)
}

/// Bounding-sphere fit: radius is the max distance from the
/// bone's rest world position to any weighted vertex, then
/// scaled.
fn fit_sphere(weighted_world: &[Vec3], bone_world: Vec3, scale: f32) -> f32 {
    let mut max_d2 = 0.0f32;
    for &p in weighted_world {
        let d2 = (p - bone_world).length_squared();
        if d2 > max_d2 {
            max_d2 = d2;
        }
    }
    max_d2.sqrt() * scale
}

/// Get the expected child bone node index for standard humanoid joints.
pub fn get_humanoid_child_node(bone_name: &str, humanoid: &HumanoidBoneRegistry) -> Option<usize> {
    let child_name = match bone_name {
        "hips" => "spine",
        "spine" => "chest",
        "chest" => {
            if humanoid.iter().any(|(b, _)| b.as_str() == "upperchest") {
                "upperchest"
            } else {
                "neck"
            }
        }
        "upperchest" => "neck",
        "neck" => "head",
        "leftupperarm" => "leftlowerarm",
        "leftlowerarm" => "lefthand",
        "rightupperarm" => "rightlowerarm",
        "rightlowerarm" => "righthand",
        "leftupperleg" => "leftlowerleg",
        "leftlowerleg" => "leftfoot",
        "rightupperleg" => "rightlowerleg",
        "rightlowerleg" => "rightfoot",
        "leftfoot" => "lefttoes",
        "rightfoot" => "righttoes",
        _ => return None,
    };
    humanoid
        .iter()
        .find(|(b, _)| b.as_str() == child_name)
        .map(|(_, e)| e.node)
}

/// Fit a capsule segment between `bone_world` and `child_world`.
#[expect(
    dead_code,
    reason = "bone name helper retained for future collider tuning"
)]
fn fit_segment_capsule(
    weighted_world: &[Vec3],
    bone_world: Vec3,
    child_world: Vec3,
    scale: f32,
) -> Option<(BoneShape, Quat, Vec3)> {
    let axis = child_world - bone_world;
    let axis_len = axis.length();
    if axis_len < 1e-6 {
        let radius = fit_sphere(weighted_world, bone_world, scale);
        return Some((BoneShape::Sphere { radius }, Quat::IDENTITY, Vec3::ZERO));
    }
    let axis_dir = axis / axis_len;

    // Radius is the max perpendicular distance to the segment.
    let mut max_r2 = 0.0f32;
    for &p in weighted_world {
        let d = p - bone_world;
        let t = (d.dot(axis_dir) / axis_len).clamp(0.0, 1.0);
        let projection = bone_world + axis_dir * (t * axis_len);
        let perp = p - projection;
        let r2 = perp.length_squared();
        if r2 > max_r2 {
            max_r2 = r2;
        }
    }
    let radius = (max_r2.sqrt() * scale).max(MIN_BONE_RADIUS);
    let half_height = 0.5 * axis_len * scale;

    let rotation = Quat::from_rotation_arc(Vec3::Y, axis_dir);
    let offset = 0.5 * axis * scale;

    Some((
        BoneShape::Capsule {
            half_height,
            radius,
        },
        rotation,
        offset,
    ))
}

/// Foot / generic "use the bone's local +Y" capsule. The
/// collider's local rotation is the bone's rest world
/// rotation; vertices in the collider-local frame are then
/// projected onto local +Y to derive the half-height.
#[expect(
    dead_code,
    reason = "bone name helper retained for future collider tuning"
)]
fn fit_bone_axis_capsule_y(
    weighted_world: &[Vec3],
    bone_world: Vec3,
    rest_world_rotations: &[Quat],
    bone_node: usize,
    scale: f32,
) -> Option<(BoneShape, Quat, Vec3)> {
    let bone_rotation = rest_world_rotations[bone_node];
    let inv = bone_rotation.inverse();
    let local: Vec<Vec3> = weighted_world
        .iter()
        .map(|p| inv * (*p - bone_world))
        .collect();
    let (shape, local_offset) = fit_y_capsule(&local, scale)?;
    let offset = bone_rotation * local_offset;
    Some((shape, bone_rotation, offset))
}

/// Project a bone-local vertex cloud onto local +Y and
/// return a `CapsuleY` covering the span (with the max
/// XZ-distance as the radius).
#[expect(
    clippy::similar_names,
    reason = "bone fitting uses min/max axis names from geometry"
)]
fn fit_y_capsule(local: &[Vec3], scale: f32) -> Option<(BoneShape, Vec3)> {
    if local.is_empty() {
        return None;
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut max_r2 = 0.0f32;
    for &p in local {
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
        let r2 = p.z.mul_add(p.z, p.x * p.x);
        if r2 > max_r2 {
            max_r2 = r2;
        }
    }
    let mid_y = f32::midpoint(min_y, max_y);
    let half_height = ((max_y - min_y) * 0.5 * scale).max(0.0);
    let radius = (max_r2.sqrt() * scale).max(MIN_BONE_RADIUS);
    let local_offset = Vec3::new(0.0, mid_y * scale, 0.0);
    Some((
        BoneShape::CapsuleY {
            half_height,
            radius,
        },
        local_offset,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_vrm::{
        BoneRestTransform, ExpressionLayer, HumanoidBoneEntry, HumanoidBoneRegistry,
        NodeConstraintRegistry, NodeHierarchy, Skeleton,
    };

    /// Build a minimal `VrmModel` with a 2-link parent→child
    /// node hierarchy and a single Skeleton joint that drives
    /// both nodes. `VrmPrimitive` is not used in the tests
    /// (the collider builder is fed directly from the
    /// `vertices: &[MeshVertex]` fixture), so the model
    /// ends up with an empty `meshes` list.
    fn model_with_two_nodes(parent_pos: Vec3, child_pos: Vec3) -> VrmModel {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            "hips".into(),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform {
                    translation: parent_pos,
                    rotation: Quat::IDENTITY,
                },
            },
        );
        humanoid.insert(
            "chest".into(),
            HumanoidBoneEntry {
                node: 1,
                joint: Some(0),
                rest: BoneRestTransform {
                    translation: child_pos,
                    rotation: Quat::IDENTITY,
                },
            },
        );
        let nodes = NodeHierarchy {
            local_rotations: vec![Quat::IDENTITY; 2],
            local_positions: vec![parent_pos, child_pos],
            rest_local_rotations: vec![Quat::IDENTITY; 2],
            rest_local_positions: vec![parent_pos, child_pos],
            parents: vec![-1, 0],
            world_rotations: vec![Quat::IDENTITY; 2],
            world_positions: vec![Vec3::ZERO; 2],
        };
        VrmModel::new(
            vec![],
            Skeleton {
                inverse_bind: vec![Mat4::IDENTITY],
                bind_matrices: vec![Mat4::IDENTITY],
                joint_to_node: vec![0],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            nodes,
            None,
            vec![],
            NodeConstraintRegistry::default(),
            None,
        )
    }

    /// Build a `MeshVertex` with all four skinning slots
    /// pointing at the same joint / weight. The test
    /// helpers above use single-slot assignments.
    fn vertex(pos: Vec3, joint: u32, weight: f32) -> MeshVertex {
        MeshVertex {
            position: pos.to_array(),
            uv: [0.0, 0.0],
            normal: [0.0, 0.0, 0.0],
            joints: [joint, 0, 0, 0],
            weights: [weight, 0.0, 0.0, 0.0],
        }
    }

    /// A vertex with `weight < VERTEX_WEIGHT_THRESHOLD` to the
    /// target joint must be excluded; the trunk's vertices are
    /// lightly weighted to nearby bones and would otherwise
    /// inflate the arm / hand capsules.
    #[test]
    fn collect_weighted_world_positions_filters_by_threshold() {
        let vertices = vec![
            vertex(Vec3::new(0.0, 1.5, 0.0), 0, 1.0),
            vertex(Vec3::new(0.0, 0.0, 0.0), 0, 0.0),
        ];
        let mut out = Vec::new();
        collect_weighted_world_positions_into(
            &vertices,
            0,
            &Mat4::IDENTITY,
            &Mat4::IDENTITY,
            &mut out,
        );
        assert_eq!(out.len(), 1, "only the weight=1.0 vertex passes the filter");
        assert!(
            (out[0].y - 1.5).abs() < 1e-5,
            "the kept vertex is at (0, 1.5, 0); got {:?}",
            out[0]
        );
    }

    /// With identity `inverse_bind` and identity
    /// `joint_world_rest`, the world position of a vertex
    /// is just its raw position.
    #[test]
    fn collect_weighted_world_positions_uses_identity_when_unskinned() {
        let vertices = vec![vertex(Vec3::new(1.0, 2.0, 3.0), 0, 1.0)];
        let mut out = Vec::new();
        collect_weighted_world_positions_into(
            &vertices,
            0,
            &Mat4::IDENTITY,
            &Mat4::IDENTITY,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], Vec3::new(1.0, 2.0, 3.0));
    }

    /// `humanoid_parent_node` must walk past non-humanoid
    /// nodes to find the closest humanoid ancestor.
    #[test]
    fn humanoid_parent_finds_nearest_humanoid_ancestor() {
        let model = model_with_two_nodes(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.5, 0.0));
        let parent = humanoid_parent_node(1, &model.nodes.parents, &model.humanoid);
        assert_eq!(parent, Some(0));
    }

    /// The top of a humanoid chain (hips) has no humanoid
    /// parent — `humanoid_parent_node` returns `None` and
    /// the limb capsule fit falls back to a sphere.
    #[test]
    fn humanoid_parent_returns_none_for_chain_root() {
        let model = model_with_two_nodes(Vec3::ZERO, Vec3::new(0.0, 0.5, 0.0));
        let parent = humanoid_parent_node(0, &model.nodes.parents, &model.humanoid);
        assert_eq!(parent, None);
    }
}
