//! VRMC_springBone-1.0 parser and per-frame simulator.
//!
//! Parses the `VRMC_springBone` extension from the glTF root
//! and provides a verlet-integration simulator that drives
//! hair / cloth / accessory sway each frame.
//!
//! ## Spec
//!
//! [VRMC_springBone-1.0](https://github.com/vrm-c/vrm-specification/tree/master/specification/VRMC_springBone-1.0)
//! defines:
//!
//! - **Colliders**: sphere or capsule shapes attached to nodes.
//! - **`ColliderGroups`**: named groups of collider indices.
//! - **Springs**: chains of joints connected head-to-tail.
//!   Each joint has `stiffness`, `gravityPower`, `gravityDir`,
//!   `dragForce`, and `hitRadius`.
//!
//! The simulator runs verlet integration per joint per frame:
//! inertia + stiffness + gravity → collision resolution →
//! rotation update.
//!
//! ## Current scope
//!
//! This module parses the JSON and provides the simulator. The
//! desktop runtime is responsible for maintaining per-node
//! world transforms and calling
//! [`SpringBoneSimulator::step`] each frame.
use std::collections::HashMap;

use glam::{Quat, Vec3};
use rayon::prelude::*;
use serde_json::Value;

/// Default hit radius (metres) when the JSON omits it.
pub const DEFAULT_HIT_RADIUS: f32 = 0.0;
/// Default stiffness (return-to-rest force).
pub const DEFAULT_STIFFNESS: f32 = 1.0;
/// Default gravity power.
pub const DEFAULT_GRAVITY_POWER: f32 = 0.0;
/// Default gravity direction (world -Y).
pub const DEFAULT_GRAVITY_DIR: [f32; 3] = [0.0, -1.0, 0.0];
/// Default drag force (deceleration, 0..1).
pub const DEFAULT_DRAG_FORCE: f32 = 0.4;

/// Shape of a collider: sphere or capsule.
#[derive(Clone, Debug)]
pub enum SpringBoneShape {
    /// Sphere collider: `offset` in the node's local space, `radius` in metres.
    Sphere {
        /// Centre of the sphere in the node's local coordinates.
        offset: [f32; 3],
        /// Radius of the sphere in metres.
        radius: f32,
    },
    /// Capsule collider: two endpoints + radius.
    Capsule {
        /// Centre of the capsule's head-side semicircle in
        /// the node's local coordinates.
        offset: [f32; 3],
        /// Radius of the capsule's cylinder and semicircles.
        radius: f32,
        /// Centre of the capsule's tail-side semicircle in
        /// the node's local coordinates.
        tail: [f32; 3],
    },
}

/// A single collider parsed from `VRMC_springBone.colliders`.
#[derive(Clone, Debug)]
pub struct SpringBoneCollider {
    /// glTF node index this collider is attached to.
    pub node: usize,
    /// Shape of the collider.
    pub shape: SpringBoneShape,
}

/// A named group of collider indices, parsed from
/// `VRMC_springBone.colliderGroups`.
#[derive(Clone, Debug)]
pub struct SpringBoneColliderGroup {
    /// Optional group name.
    pub name: Option<String>,
    /// Indices into [`SpringBoneProperties::colliders`].
    pub collider_indices: Vec<usize>,
}

/// A single joint in a spring chain, parsed from
/// `VRMC_springBone.springs[*].joints[*]`.
#[derive(Clone, Debug)]
pub struct SpringBoneJoint {
    /// glTF node index of this joint.
    pub node: usize,
    /// Collision sphere radius (metres). [`DEFAULT_HIT_RADIUS`]
    /// when omitted.
    pub hit_radius: f32,
    /// Force to return to the rest orientation.
    /// [`DEFAULT_STIFFNESS`] when omitted.
    pub stiffness: f32,
    /// Gravity magnitude. [`DEFAULT_GRAVITY_POWER`] when omitted.
    pub gravity_power: f32,
    /// Gravity direction (world space). [`DEFAULT_GRAVITY_DIR`]
    /// when omitted.
    pub gravity_dir: [f32; 3],
    /// Deceleration factor in `[0, 1]`. [`DEFAULT_DRAG_FORCE`]
    /// when omitted.
    pub drag_force: f32,
}

/// A single spring chain, parsed from
/// `VRMC_springBone.springs[*]`.
#[derive(Clone, Debug)]
pub struct SpringBoneChain {
    /// Optional chain name.
    pub name: Option<String>,
    /// Joints in head-to-tail order. `joints[n]` must be an
    /// ancestor of `joints[n+1]` in the glTF node hierarchy.
    pub joints: Vec<SpringBoneJoint>,
    /// Indices into [`SpringBoneProperties::collider_groups`]
    /// that this chain collides against.
    pub collider_group_indices: Vec<usize>,
    /// Optional center node index. When set, the chain's
    /// inertia is evaluated in the center node's local space
    /// instead of world space.
    pub center: Option<usize>,
}

/// Top-level spring bone properties parsed from
/// `VRMC_springBone`.
#[derive(Clone, Debug)]
pub struct SpringBoneProperties {
    /// Spec version string (should be `"1.0"`).
    pub spec_version: String,
    /// All colliders in the model.
    pub colliders: Vec<SpringBoneCollider>,
    /// Named collider groups.
    pub collider_groups: Vec<SpringBoneColliderGroup>,
    /// Spring chains.
    pub springs: Vec<SpringBoneChain>,
}

/// Parse `VRMC_springBone` from the glTF root extensions.
///
/// Returns `None` if the extension is not present. Logs
/// warnings for malformed entries and falls back to defaults.
///
/// Called internally by [`crate::loader::load_vrm`]; hidden from the "Supported API" docs —
/// hosts get the result via [`VrmModel::spring_bones`](crate::model::VrmModel).
#[doc(hidden)]
#[must_use]
pub fn load_spring_bones(gltf: &gltf::Gltf) -> Option<SpringBoneProperties> {
    let ext = gltf.document.extensions()?;
    let value = ext.get("VRMC_springBone")?;

    let spec_version = value
        .get("specVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0")
        .to_string();

    let colliders = parse_colliders(value);
    let collider_groups = parse_collider_groups(value);
    let springs = parse_springs(value);

    Some(SpringBoneProperties {
        spec_version,
        colliders,
        collider_groups,
        springs,
    })
}

fn parse_colliders(value: &Value) -> Vec<SpringBoneCollider> {
    let Some(arr) = value.get("colliders").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(|c| {
            let node = c.get("node")?.as_u64()? as usize;
            let shape_obj = c.get("shape")?;

            let shape = if let Some(sphere) = shape_obj.get("sphere") {
                let offset = parse_vec3(sphere.get("offset")?).unwrap_or([0.0, 0.0, 0.0]);
                let radius = sphere
                    .get("radius")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0) as f32;
                SpringBoneShape::Sphere { offset, radius }
            } else if let Some(capsule) = shape_obj.get("capsule") {
                let offset = parse_vec3(capsule.get("offset")?).unwrap_or([0.0, 0.0, 0.0]);
                let radius = capsule
                    .get("radius")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0) as f32;
                let tail = parse_vec3(capsule.get("tail")?).unwrap_or([0.0, 0.0, 0.0]);
                SpringBoneShape::Capsule {
                    offset,
                    radius,
                    tail,
                }
            } else {
                tracing::warn!("springBone collider missing shape; skipping");
                return None;
            };

            Some(SpringBoneCollider { node, shape })
        })
        .collect()
}

fn parse_collider_groups(value: &Value) -> Vec<SpringBoneColliderGroup> {
    let Some(arr) = value.get("colliderGroups").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    arr.iter()
        .map(|g| {
            let name = g.get("name").and_then(|v| v.as_str()).map(String::from);
            let collider_indices = g
                .get("colliders")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_u64().map(|i| i as usize))
                        .collect()
                })
                .unwrap_or_default();
            SpringBoneColliderGroup {
                name,
                collider_indices,
            }
        })
        .collect()
}

fn parse_springs(value: &Value) -> Vec<SpringBoneChain> {
    let Some(arr) = value.get("springs").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    arr.iter()
        .map(|s| {
            let name = s.get("name").and_then(|v| v.as_str()).map(String::from);
            let joints = s
                .get("joints")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|j| {
                            let node = j.get("node")?.as_u64()? as usize;
                            let hit_radius = j
                                .get("hitRadius")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or_else(|| f64::from(DEFAULT_HIT_RADIUS))
                                as f32;
                            let stiffness = j
                                .get("stiffness")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or_else(|| f64::from(DEFAULT_STIFFNESS))
                                as f32;
                            let gravity_power = j
                                .get("gravityPower")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or_else(|| f64::from(DEFAULT_GRAVITY_POWER))
                                as f32;
                            let gravity_dir = j
                                .get("gravityDir")
                                .and_then(parse_vec3)
                                .unwrap_or(DEFAULT_GRAVITY_DIR);
                            let drag_force = j
                                .get("dragForce")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or_else(|| f64::from(DEFAULT_DRAG_FORCE))
                                as f32;
                            Some(SpringBoneJoint {
                                node,
                                hit_radius,
                                stiffness,
                                gravity_power,
                                gravity_dir,
                                drag_force,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let collider_group_indices = s
                .get("colliderGroups")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_u64().map(|i| i as usize))
                        .collect()
                })
                .unwrap_or_default();
            let center = s
                .get("center")
                .and_then(serde_json::Value::as_u64)
                .map(|i| i as usize);
            SpringBoneChain {
                name,
                joints,
                collider_group_indices,
                center,
            }
        })
        .collect()
}

fn parse_vec3(v: &Value) -> Option<[f32; 3]> {
    let arr = v.as_array()?;
    if arr.len() < 3 {
        return None;
    }
    Some([
        arr[0].as_f64()? as f32,
        arr[1].as_f64()? as f32,
        arr[2].as_f64()? as f32,
    ])
}

/// Per-joint runtime state for the verlet integrator.
#[derive(Clone, Debug)]
pub struct SpringBoneJointState {
    /// Previous frame's tail position (world or center space).
    pub prev_tail: Vec3,
    /// Current frame's tail position.
    pub current_tail: Vec3,
    /// Direction from this joint to its child in the joint's
    /// local rest frame.
    pub bone_axis: Vec3,
    /// Length of the bone segment (world or center space).
    pub bone_length: f32,
    /// Rest rotation of this joint node.
    pub initial_local_rotation: Quat,
}

/// Per-chain runtime state.
#[derive(Clone, Debug)]
pub struct SpringBoneChainState {
    /// Per-joint state, parallel to [`SpringBoneChain::joints`].
    pub joints: Vec<SpringBoneJointState>,
}

/// Simulator for all spring chains in the model.
///
/// Created once after loading; call [`step`](Self::step) each
/// frame with the current node transforms.
#[derive(Clone, Debug)]
pub struct SpringBoneSimulator {
    /// Per-chain state, parallel to
    /// [`SpringBoneProperties::springs`].
    pub chains: Vec<SpringBoneChainState>,
}

impl SpringBoneSimulator {
    /// Create a new simulator from the parsed properties and
    /// the initial node world transforms.
    ///
    /// `node_world_positions` maps glTF node index to world
    /// position. `node_world_rotations` maps glTF node index to
    /// world rotation. `node_parent_world_rotations` maps glTF
    /// node index to its parent's world rotation (identity for
    /// root nodes).
    ///
    /// `node_local_rotations` maps glTF node index to the node's
    /// rest-pose local rotation (from the glTF file).
    #[must_use]
    pub fn new(
        props: &SpringBoneProperties,
        node_world_positions: &HashMap<usize, Vec3>,
        node_world_rotations: &HashMap<usize, Quat>,
        node_parent_world_rotations: &HashMap<usize, Quat>,
        node_local_rotations: &HashMap<usize, Quat>,
    ) -> Self {
        let chains = props
            .springs
            .iter()
            .map(|chain| {
                let joints = chain
                    .joints
                    .iter()
                    .enumerate()
                    .map(|(i, joint)| {
                        let world_pos = node_world_positions
                            .get(&joint.node)
                            .copied()
                            .unwrap_or(Vec3::ZERO);
                        let world_rot = node_world_rotations
                            .get(&joint.node)
                            .copied()
                            .unwrap_or(Quat::IDENTITY);
                        let parent_world_rot = node_parent_world_rotations
                            .get(&joint.node)
                            .copied()
                            .unwrap_or(Quat::IDENTITY);
                        let local_rot = node_local_rotations
                            .get(&joint.node)
                            .copied()
                            .unwrap_or(Quat::IDENTITY);

                        // bone_axis: direction to child in local space
                        // bone_length: distance to child in world space
                        let (bone_axis, bone_length) = if i + 1 < chain.joints.len() {
                            let child_node = chain.joints[i + 1].node;
                            let child_world_pos = node_world_positions
                                .get(&child_node)
                                .copied()
                                .unwrap_or(Vec3::ZERO);
                            let child_local = world_rot.inverse() * (child_world_pos - world_pos);
                            let axis = if child_local.length_squared() > 1e-10 {
                                child_local.normalize()
                            } else {
                                Vec3::Y
                            };
                            let len = (child_world_pos - world_pos).length();
                            (axis, len)
                        } else {
                            // Tail joint: use parent's world rotation to
                            // estimate a downward axis
                            let axis = parent_world_rot.inverse() * (-Vec3::Y);
                            let len = 0.07; // vrm0 fallback: 7 cm
                            (axis, len)
                        };

                        // current_tail: child's world position at rest
                        let current_tail = world_pos + world_rot * (bone_axis * bone_length);

                        SpringBoneJointState {
                            prev_tail: current_tail,
                            current_tail,
                            bone_axis,
                            bone_length,
                            initial_local_rotation: local_rot,
                        }
                    })
                    .collect();
                SpringBoneChainState { joints }
            })
            .collect();

        Self { chains }
    }

    /// Advance the simulation by `dt` seconds.
    ///
    /// `node_world_positions` and `node_world_rotations` are the
    /// current per-node world transforms. `node_parent_world_rotations`
    /// are the parent's world rotations (identity for roots).
    ///
    /// `collider_world_positions` maps collider node index to
    /// world position. `collider_world_rotations` maps collider
    /// node index to world rotation.
    ///
    /// Returns a map from glTF node index to the updated local
    /// rotation for each joint that moved.
    pub fn step(
        &mut self,
        dt: f32,
        props: &SpringBoneProperties,
        node_world_positions: &HashMap<usize, Vec3>,
        node_world_rotations: &HashMap<usize, Quat>,
        node_parent_world_rotations: &HashMap<usize, Quat>,
        collider_world_positions: &HashMap<usize, Vec3>,
        collider_world_rotations: &HashMap<usize, Quat>,
    ) -> HashMap<usize, Quat> {
        let thread_results: Vec<HashMap<usize, Quat>> = props
            .springs
            .par_iter()
            .zip(self.chains.par_iter_mut())
            .map(|(chain, chain_state)| {
                let mut updated_rotations = HashMap::new();

                // Collect colliders for this chain
                let mut chain_colliders: Vec<(usize, &SpringBoneShape)> = Vec::new();
                for &cg_idx in &chain.collider_group_indices {
                    if let Some(cg) = props.collider_groups.get(cg_idx) {
                        for &c_idx in &cg.collider_indices {
                            if let Some(collider) = props.colliders.get(c_idx) {
                                chain_colliders.push((collider.node, &collider.shape));
                            }
                        }
                    }
                }

                // Center space transform (if set)
                let center_world_pos = chain
                    .center
                    .and_then(|c| node_world_positions.get(&c).copied());
                let center_world_rot = chain
                    .center
                    .and_then(|c| node_world_rotations.get(&c).copied());

                for (joint_idx, joint) in chain.joints.iter().enumerate() {
                    // Skip the last joint (it's only a tail target)
                    if joint_idx + 1 >= chain.joints.len() {
                        break;
                    }

                    let state = &mut chain_state.joints[joint_idx];

                    let world_pos = node_world_positions
                        .get(&joint.node)
                        .copied()
                        .unwrap_or(Vec3::ZERO);
                    let parent_world_rot = node_parent_world_rotations
                        .get(&joint.node)
                        .copied()
                        .unwrap_or(Quat::IDENTITY);

                    // Transform to center space if needed
                    let (world_pos, parent_world_rot) =
                        if let (Some(c_pos), Some(c_rot)) = (center_world_pos, center_world_rot) {
                            let local_pos = c_rot.inverse() * (world_pos - c_pos);
                            let local_rot = c_rot.inverse() * parent_world_rot;
                            (local_pos, local_rot)
                        } else {
                            (world_pos, parent_world_rot)
                        };

                    // Verlet integration
                    let drag = joint.drag_force.clamp(0.0, 1.0);
                    let inertia = (state.current_tail - state.prev_tail) * (1.0 - drag);

                    let stiffness_dir =
                        parent_world_rot * state.initial_local_rotation * state.bone_axis;
                    let stiffness = stiffness_dir * joint.stiffness * dt;

                    let gravity = Vec3::from(joint.gravity_dir) * joint.gravity_power * dt;

                    let mut next_tail = state.current_tail + inertia + stiffness + gravity;

                    // Constrain bone length
                    let to_tail = next_tail - world_pos;
                    if to_tail.length_squared() > 1e-10 {
                        next_tail = world_pos + to_tail.normalize() * state.bone_length;
                    } else {
                        next_tail = world_pos + stiffness_dir * state.bone_length;
                    }

                    // Collision resolution
                    let joint_radius = joint.hit_radius;
                    for &(collider_node, shape) in &chain_colliders {
                        let collider_pos = collider_world_positions
                            .get(&collider_node)
                            .copied()
                            .unwrap_or(Vec3::ZERO);
                        let collider_rot = collider_world_rotations
                            .get(&collider_node)
                            .copied()
                            .unwrap_or(Quat::IDENTITY);

                        let (distance, direction) = match shape {
                            SpringBoneShape::Sphere { offset, radius } => {
                                let world_offset = collider_rot * Vec3::from(*offset);
                                let sphere_center = collider_pos + world_offset;
                                let delta = next_tail - sphere_center;
                                let dist = delta.length() - radius - joint_radius;
                                let dir = if delta.length_squared() > 1e-10 {
                                    delta.normalize()
                                } else {
                                    Vec3::Y
                                };
                                (dist, dir)
                            }
                            SpringBoneShape::Capsule {
                                offset,
                                radius,
                                tail,
                            } => {
                                let world_offset = collider_rot * Vec3::from(*offset);
                                let world_tail = collider_rot * Vec3::from(*tail);
                                let capsule_start = collider_pos + world_offset;
                                let capsule_end = collider_pos + world_tail;
                                let capsule_axis = capsule_end - capsule_start;
                                let axis_len_sq = capsule_axis.length_squared();

                                let delta = next_tail - capsule_start;
                                let t = if axis_len_sq > 1e-10 {
                                    (delta.dot(capsule_axis) / axis_len_sq).clamp(0.0, 1.0)
                                } else {
                                    0.0
                                };

                                let closest = capsule_start + capsule_axis * t;
                                let diff = next_tail - closest;
                                let dist = diff.length() - radius - joint_radius;
                                let dir = if diff.length_squared() > 1e-10 {
                                    diff.normalize()
                                } else {
                                    Vec3::Y
                                };
                                (dist, dir)
                            }
                        };

                        if distance < 0.0 {
                            next_tail -= direction * distance;
                            // Re-constrain bone length
                            let to_tail = next_tail - world_pos;
                            if to_tail.length_squared() > 1e-10 {
                                next_tail = world_pos + to_tail.normalize() * state.bone_length;
                            }
                        }
                    }

                    // Update state
                    state.prev_tail = state.current_tail;
                    state.current_tail = next_tail;

                    // Compute rotation
                    let parent_world_matrix = parent_world_rot;
                    let to_local =
                        (parent_world_matrix.inverse() * (next_tail - world_pos)).normalize();
                    let from_to = Quat::from_rotation_arc(state.bone_axis, to_local);
                    let new_rotation = state.initial_local_rotation * from_to;

                    updated_rotations.insert(joint.node, new_rotation);
                }

                updated_rotations
            })
            .collect();

        let mut updated_rotations = HashMap::new();
        for res in thread_results {
            updated_rotations.extend(res);
        }

        updated_rotations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gltf_from_vrmc(vrmc: Value) -> gltf::Gltf {
        let mut root = serde_json::Map::new();
        root.insert("asset".into(), serde_json::json!({"version": "2.0"}));
        root.insert(
            "extensionsUsed".into(),
            serde_json::json!(["VRMC_springBone"]),
        );
        root.insert(
            "extensions".into(),
            serde_json::json!({"VRMC_springBone": vrmc}),
        );
        root.insert("nodes".into(), serde_json::json!([]));
        let bytes = serde_json::to_vec(&root).unwrap();
        gltf::Gltf::from_slice(&bytes).unwrap()
    }

    #[test]
    fn parse_empty_spring_bones() {
        let vrmc = serde_json::json!({
            "specVersion": "1.0",
            "colliders": [],
            "colliderGroups": [],
            "springs": []
        });
        let gltf = gltf_from_vrmc(vrmc);
        let props = load_spring_bones(&gltf).unwrap();
        assert_eq!(props.spec_version, "1.0");
        assert!(props.colliders.is_empty());
        assert!(props.collider_groups.is_empty());
        assert!(props.springs.is_empty());
    }

    #[test]
    fn parse_sphere_collider() {
        let vrmc = serde_json::json!({
            "specVersion": "1.0",
            "colliders": [
                {
                    "node": 5,
                    "shape": {
                        "sphere": {
                            "offset": [0.1, 0.2, 0.3],
                            "radius": 0.5
                        }
                    }
                }
            ],
            "colliderGroups": [],
            "springs": []
        });
        let gltf = gltf_from_vrmc(vrmc);
        let props = load_spring_bones(&gltf).unwrap();
        assert_eq!(props.colliders.len(), 1);
        assert_eq!(props.colliders[0].node, 5);
        match &props.colliders[0].shape {
            SpringBoneShape::Sphere { offset, radius } => {
                assert!((offset[0] - 0.1).abs() < 1e-6);
                assert!((offset[1] - 0.2).abs() < 1e-6);
                assert!((offset[2] - 0.3).abs() < 1e-6);
                assert!((radius - 0.5).abs() < 1e-6);
            }
            _ => panic!("expected Sphere"),
        }
    }

    #[test]
    fn parse_capsule_collider() {
        let vrmc = serde_json::json!({
            "specVersion": "1.0",
            "colliders": [
                {
                    "node": 3,
                    "shape": {
                        "capsule": {
                            "offset": [0, 0, 0],
                            "radius": 0.1,
                            "tail": [0, 0, 1]
                        }
                    }
                }
            ],
            "colliderGroups": [],
            "springs": []
        });
        let gltf = gltf_from_vrmc(vrmc);
        let props = load_spring_bones(&gltf).unwrap();
        assert_eq!(props.colliders.len(), 1);
        match &props.colliders[0].shape {
            SpringBoneShape::Capsule {
                offset,
                radius,
                tail,
            } => {
                assert!((radius - 0.1).abs() < 1e-6);
                assert!((tail[2] - 1.0).abs() < 1e-6);
                let _ = offset;
            }
            _ => panic!("expected Capsule"),
        }
    }

    #[test]
    fn parse_collider_group() {
        let vrmc = serde_json::json!({
            "specVersion": "1.0",
            "colliders": [],
            "colliderGroups": [
                {
                    "name": "body",
                    "colliders": [0, 1, 2]
                }
            ],
            "springs": []
        });
        let gltf = gltf_from_vrmc(vrmc);
        let props = load_spring_bones(&gltf).unwrap();
        assert_eq!(props.collider_groups.len(), 1);
        assert_eq!(props.collider_groups[0].name.as_deref(), Some("body"));
        assert_eq!(props.collider_groups[0].collider_indices, vec![0, 1, 2]);
    }

    #[test]
    fn parse_spring_chain_with_defaults() {
        let vrmc = serde_json::json!({
            "specVersion": "1.0",
            "colliders": [],
            "colliderGroups": [],
            "springs": [
                {
                    "name": "hair_front",
                    "joints": [
                        {"node": 10},
                        {"node": 11},
                        {"node": 12}
                    ],
                    "colliderGroups": [0]
                }
            ]
        });
        let gltf = gltf_from_vrmc(vrmc);
        let props = load_spring_bones(&gltf).unwrap();
        assert_eq!(props.springs.len(), 1);
        assert_eq!(props.springs[0].name.as_deref(), Some("hair_front"));
        assert_eq!(props.springs[0].joints.len(), 3);
        assert_eq!(props.springs[0].joints[0].node, 10);
        assert_eq!(props.springs[0].joints[1].node, 11);
        assert_eq!(props.springs[0].joints[2].node, 12);
        // Defaults
        assert!((props.springs[0].joints[0].hit_radius - DEFAULT_HIT_RADIUS).abs() < 1e-6);
        assert!((props.springs[0].joints[0].stiffness - DEFAULT_STIFFNESS).abs() < 1e-6);
        assert!((props.springs[0].joints[0].gravity_power - DEFAULT_GRAVITY_POWER).abs() < 1e-6);
        assert!((props.springs[0].joints[0].drag_force - DEFAULT_DRAG_FORCE).abs() < 1e-6);
    }

    #[test]
    fn parse_spring_chain_with_custom_params() {
        let vrmc = serde_json::json!({
            "specVersion": "1.0",
            "colliders": [],
            "colliderGroups": [],
            "springs": [
                {
                    "joints": [
                        {
                            "node": 20,
                            "hitRadius": 0.05,
                            "stiffness": 0.8,
                            "gravityPower": 1.5,
                            "gravityDir": [0, -1, 0.1],
                            "dragForce": 0.3
                        },
                        {"node": 21}
                    ],
                    "colliderGroups": [],
                    "center": 5
                }
            ]
        });
        let gltf = gltf_from_vrmc(vrmc);
        let props = load_spring_bones(&gltf).unwrap();
        let joint = &props.springs[0].joints[0];
        assert!((joint.hit_radius - 0.05).abs() < 1e-6);
        assert!((joint.stiffness - 0.8).abs() < 1e-6);
        assert!((joint.gravity_power - 1.5).abs() < 1e-6);
        assert!((joint.gravity_dir[2] - 0.1).abs() < 1e-6);
        assert!((joint.drag_force - 0.3).abs() < 1e-6);
        assert_eq!(props.springs[0].center, Some(5));
    }

    #[test]
    fn no_extension_returns_none() {
        let root = serde_json::json!({
            "asset": {"version": "2.0"},
            "nodes": []
        });
        let bytes = serde_json::to_vec(&root).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        assert!(load_spring_bones(&gltf).is_none());
    }

    #[test]
    fn simulator_initializes_from_rest() {
        let props = SpringBoneProperties {
            spec_version: "1.0".into(),
            colliders: vec![],
            collider_groups: vec![],
            springs: vec![SpringBoneChain {
                name: None,
                joints: vec![
                    SpringBoneJoint {
                        node: 0,
                        hit_radius: 0.0,
                        stiffness: 1.0,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    },
                    SpringBoneJoint {
                        node: 1,
                        hit_radius: 0.0,
                        stiffness: 1.0,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    },
                ],
                collider_group_indices: vec![],
                center: None,
            }],
        };

        let mut positions = HashMap::new();
        positions.insert(0, Vec3::new(0.0, 1.0, 0.0));
        positions.insert(1, Vec3::new(0.0, 0.5, 0.0));

        let rotations = HashMap::new();
        let parent_rotations = HashMap::new();
        let local_rotations = HashMap::new();

        let sim = SpringBoneSimulator::new(
            &props,
            &positions,
            &rotations,
            &parent_rotations,
            &local_rotations,
        );

        assert_eq!(sim.chains.len(), 1);
        assert_eq!(sim.chains[0].joints.len(), 2);
        // First joint's current_tail should be at child's rest position
        let tail = sim.chains[0].joints[0].current_tail;
        assert!((tail - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-5);
    }

    #[test]
    fn simulator_step_no_forces_stays_at_rest() {
        let props = SpringBoneProperties {
            spec_version: "1.0".into(),
            colliders: vec![],
            collider_groups: vec![],
            springs: vec![SpringBoneChain {
                name: None,
                joints: vec![
                    SpringBoneJoint {
                        node: 0,
                        hit_radius: 0.0,
                        stiffness: 1.0,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    },
                    SpringBoneJoint {
                        node: 1,
                        hit_radius: 0.0,
                        stiffness: 1.0,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    },
                ],
                collider_group_indices: vec![],
                center: None,
            }],
        };

        let mut positions = HashMap::new();
        positions.insert(0, Vec3::new(0.0, 1.0, 0.0));
        positions.insert(1, Vec3::new(0.0, 0.5, 0.0));

        let rotations = HashMap::new();
        let parent_rotations = HashMap::new();
        let local_rotations = HashMap::new();

        let mut sim = SpringBoneSimulator::new(
            &props,
            &positions,
            &rotations,
            &parent_rotations,
            &local_rotations,
        );

        let collider_positions = HashMap::new();
        let collider_rotations = HashMap::new();

        let updated = sim.step(
            1.0 / 60.0,
            &props,
            &positions,
            &rotations,
            &parent_rotations,
            &collider_positions,
            &collider_rotations,
        );

        // With no gravity and stiffness=1, the joint should stay near rest
        assert!(updated.contains_key(&0));
        let new_rot = updated[&0];
        // Rotation should be near identity (no forces applied)
        assert!(new_rot.angle_between(Quat::IDENTITY) < 0.1);
    }

    #[test]
    fn simulator_step_updates_state() {
        let props = SpringBoneProperties {
            spec_version: "1.0".into(),
            colliders: vec![],
            collider_groups: vec![],
            springs: vec![SpringBoneChain {
                name: None,
                joints: vec![
                    SpringBoneJoint {
                        node: 0,
                        hit_radius: 0.0,
                        stiffness: 1.0,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    },
                    SpringBoneJoint {
                        node: 1,
                        hit_radius: 0.0,
                        stiffness: 1.0,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    },
                ],
                collider_group_indices: vec![],
                center: None,
            }],
        };

        let mut positions = HashMap::new();
        positions.insert(0, Vec3::new(0.0, 1.0, 0.0));
        positions.insert(1, Vec3::new(0.0, 0.5, 0.0));

        let rotations = HashMap::new();
        let parent_rotations = HashMap::new();
        let local_rotations = HashMap::new();

        let mut sim = SpringBoneSimulator::new(
            &props,
            &positions,
            &rotations,
            &parent_rotations,
            &local_rotations,
        );

        let collider_positions = HashMap::new();
        let collider_rotations = HashMap::new();

        let updated = sim.step(
            1.0 / 60.0,
            &props,
            &positions,
            &rotations,
            &parent_rotations,
            &collider_positions,
            &collider_rotations,
        );

        assert!(updated.contains_key(&0));
    }
}
