//! VRMC_node_constraint-1.0 parser and evaluator.
//!
//! Parses the `VRMC_node_constraint` extension from glTF nodes
//! and provides a per-frame evaluator that applies rotation
//! constraints (aim, roll, rotation) to constrained bones.
//!
//! ## Spec
//!
//! [`VRMC_node_constraint-1.0`](https://github.com/vrm-c/vrm-specification/tree/master/specification/VRMC_node_constraint-1.0)
//! defines three constraint types:
//!
//! - **Rotation**: copies the source's rotation delta onto the
//!   destination (Local-Local space).
//! - **Roll**: extracts the rotation component around a
//!   specified axis from the source and applies it to the
//!   destination (for twist bones).
//! - **Aim**: rotates the destination so a specified local axis
//!   points at the source's world position.
//!
//! All three use slerp with `weight` to blend between the
//! destination's rest rotation and the fully-constrained target.
//!
//! ## Current scope
//!
//! This module parses the constraint JSON and provides the
//! evaluator. The desktop runtime is responsible for maintaining
//! per-node transform state and calling
//! [`NodeConstraintRegistry::evaluate`] each frame.
use std::collections::HashMap;

use glam::{Quat, Vec3};
use serde_json::Value;

/// Roll axis for [`NodeConstraint::Roll`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RollAxis {
    X,
    Y,
    Z,
}

impl RollAxis {
    const fn as_vec3(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
    }

    fn from_json_str(s: &str) -> Option<Self> {
        match s {
            "X" => Some(Self::X),
            "Y" => Some(Self::Y),
            "Z" => Some(Self::Z),
            _ => None,
        }
    }
}

/// Aim axis for [`NodeConstraint::Aim`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AimAxis {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
}

impl AimAxis {
    fn as_vec3(self) -> Vec3 {
        match self {
            Self::PositiveX => Vec3::X,
            Self::NegativeX => -Vec3::X,
            Self::PositiveY => Vec3::Y,
            Self::NegativeY => -Vec3::Y,
            Self::PositiveZ => Vec3::Z,
            Self::NegativeZ => -Vec3::Z,
        }
    }

    fn from_json_str(s: &str) -> Option<Self> {
        match s {
            "PositiveX" => Some(Self::PositiveX),
            "NegativeX" => Some(Self::NegativeX),
            "PositiveY" => Some(Self::PositiveY),
            "NegativeY" => Some(Self::NegativeY),
            "PositiveZ" => Some(Self::PositiveZ),
            "NegativeZ" => Some(Self::NegativeZ),
            _ => None,
        }
    }
}

/// A single node constraint parsed from `VRMC_node_constraint`.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum NodeConstraint {
    /// Copy the source's rotation delta onto the destination.
    Rotation {
        /// glTF node index of the source (driver) bone.
        source_node: usize,
        /// Blend weight in `[0.0, 1.0]`.
        weight: f32,
    },
    /// Extract the rotation component around `roll_axis` from
    /// the source and apply it to the destination.
    Roll {
        /// glTF node index of the source (driver) bone.
        source_node: usize,
        /// Which local axis of the destination to roll on.
        roll_axis: RollAxis,
        /// Blend weight in `[0.0, 1.0]`.
        weight: f32,
    },
    /// Rotate the destination so `aim_axis` points at the
    /// source's world position.
    Aim {
        /// glTF node index of the source (driver) bone.
        source_node: usize,
        /// Which local axis of the destination should point at the source.
        aim_axis: AimAxis,
        /// Blend weight in `[0.0, 1.0]`.
        weight: f32,
    },
}

impl NodeConstraint {
    pub const fn source_node(&self) -> usize {
        match self {
            Self::Rotation { source_node, .. }
            | Self::Roll { source_node, .. }
            | Self::Aim { source_node, .. } => *source_node,
        }
    }

    pub const fn weight(&self) -> f32 {
        match self {
            Self::Rotation { weight, .. }
            | Self::Roll { weight, .. }
            | Self::Aim { weight, .. } => *weight,
        }
    }
}

/// A single constraint entry: which node is constrained and by what.
#[derive(Clone, Debug)]
pub struct ConstraintEntry {
    /// The glTF node index of the destination (driven) bone.
    pub dest_node: usize,
    pub constraint: NodeConstraint,
}

/// Registry of all node constraints in the model.
///
/// Built once at load time by [`load_node_constraints`]. The
/// desktop runtime calls [`evaluate`](Self::evaluate) each
/// frame to update constrained bone rotations.
#[derive(Clone, Debug, Default)]
pub struct NodeConstraintRegistry {
    /// All constraints in the model, in declaration order.
    pub entries: Vec<ConstraintEntry>,
}

impl NodeConstraintRegistry {
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Evaluate all constraints and return the updated local
    /// rotations for constrained destination nodes.
    ///
    /// `node_local_rotations` maps glTF node index to the
    /// current local rotation. The evaluator reads source
    /// rotations from this map and writes destination rotations
    /// back into it.
    ///
    /// `node_rest_rotations` maps glTF node index to the rest
    /// pose local rotation (the node's initial rotation in the
    /// glTF file).
    ///
    /// `node_world_positions` maps glTF node index to the
    /// current world-space position. Required for Aim
    /// constraints.
    ///
    /// `node_parent_world_rotations` maps glTF node index to
    /// the parent's current world-space rotation. Required for
    /// Aim constraints.
    ///
    /// Returns a `HashMap<dest_node, new_local_rotation>` with
    /// the updated rotations for every constrained destination.
    /// The caller is responsible for applying these to the
    /// skeleton / skin palette.
    pub fn evaluate(
        &self,
        node_local_rotations: &HashMap<usize, Quat>,
        node_rest_rotations: &HashMap<usize, Quat>,
        node_world_positions: &HashMap<usize, Vec3>,
        node_parent_world_rotations: &HashMap<usize, Quat>,
    ) -> HashMap<usize, Quat> {
        let mut results = HashMap::new();

        for entry in &self.entries {
            let dest = entry.dest_node;
            let constraint = &entry.constraint;
            let source = constraint.source_node();
            let weight = constraint.weight();

            let Some(&src_local) = node_local_rotations.get(&source) else {
                continue;
            };
            let Some(&src_rest) = node_rest_rotations.get(&source) else {
                continue;
            };
            let Some(&dst_rest) = node_rest_rotations.get(&dest) else {
                continue;
            };

            let new_rot = match constraint {
                NodeConstraint::Rotation { .. } => {
                    Self::eval_rotation(src_local, src_rest, dst_rest, weight)
                }
                NodeConstraint::Roll { roll_axis, .. } => {
                    Self::eval_roll(src_local, src_rest, dst_rest, *roll_axis, weight)
                }
                NodeConstraint::Aim { aim_axis, .. } => {
                    let Some(&src_world_pos) = node_world_positions.get(&source) else {
                        continue;
                    };
                    let Some(&dst_world_pos) = node_world_positions.get(&dest) else {
                        continue;
                    };
                    let Some(&dst_parent_world_rot) = node_parent_world_rotations.get(&dest) else {
                        continue;
                    };
                    Self::eval_aim(
                        src_world_pos,
                        dst_world_pos,
                        dst_rest,
                        dst_parent_world_rot,
                        *aim_axis,
                        weight,
                    )
                }
            };

            results.insert(dest, new_rot);
        }

        results
    }

    /// Rotation constraint: copy the source's rotation delta
    /// onto the destination.
    ///
    /// `new_rot = slerp(dst_rest, dst_rest * (src_rest^-1 * src_local), weight)`
    fn eval_rotation(src_local: Quat, src_rest: Quat, dst_rest: Quat, weight: f32) -> Quat {
        let src_delta = src_rest.inverse() * src_local;
        let target = dst_rest * src_delta;
        dst_rest.slerp(target, weight)
    }

    /// Roll constraint: extract the rotation component around
    /// `roll_axis` from the source and apply it to the
    /// destination.
    fn eval_roll(
        src_local: Quat,
        src_rest: Quat,
        dst_rest: Quat,
        roll_axis: RollAxis,
        weight: f32,
    ) -> Quat {
        let axis_vec = roll_axis.as_vec3();
        let src_delta = src_rest.inverse() * src_local;
        let delta_in_parent = dst_rest * src_delta * src_rest.inverse();
        let delta_in_dest = dst_rest.inverse() * delta_in_parent * dst_rest;
        let to_vec = delta_in_dest * axis_vec;
        let from_to = Quat::from_rotation_arc(axis_vec, to_vec);
        let target = dst_rest * from_to.inverse() * delta_in_parent;
        dst_rest.slerp(target, weight)
    }

    /// Aim constraint: rotate the destination so `aim_axis`
    /// points at the source's world position.
    fn eval_aim(
        src_world_pos: Vec3,
        dst_world_pos: Vec3,
        dst_rest: Quat,
        dst_parent_world_rot: Quat,
        aim_axis: AimAxis,
        weight: f32,
    ) -> Quat {
        let axis_vec = aim_axis.as_vec3();
        let from_vec = dst_parent_world_rot * dst_rest * axis_vec;
        let to_vec = (src_world_pos - dst_world_pos).normalize_or_zero();
        if to_vec.length_squared() < 1e-10 {
            return dst_rest;
        }
        let from_to = Quat::from_rotation_arc(from_vec, to_vec);
        let target = dst_parent_world_rot.inverse() * from_to * dst_parent_world_rot * dst_rest;
        dst_rest.slerp(target, weight)
    }
}

/// Parse `VRMC_node_constraint` from every glTF node and
/// return the registry.
///
/// Walks `gltf.document.nodes()`, checks each node's
/// `extensions` for `VRMC_node_constraint`, and parses the
/// constraint JSON. Returns an empty registry for models
/// without the extension.
///
/// Called internally by [`crate::loader::load_vrm`]; hidden from the "Supported API" docs —
/// hosts get the registry via [`VrmModel::node_constraints`](crate::model::VrmModel).
#[doc(hidden)]
pub fn load_node_constraints(gltf: &gltf::Gltf) -> NodeConstraintRegistry {
    let mut entries = Vec::new();

    let Some(root_ext) = gltf.document.extensions() else {
        return NodeConstraintRegistry::default();
    };
    if root_ext.get("VRMC_node_constraint").is_none() {
        return NodeConstraintRegistry::default();
    }

    for (node_idx, node) in gltf.document.nodes().enumerate() {
        let Some(node_ext) = node.extensions() else {
            continue;
        };
        let Some(constraint_json) = node_ext.get("VRMC_node_constraint") else {
            continue;
        };

        match parse_constraint(constraint_json) {
            Some(constraint) => {
                entries.push(ConstraintEntry {
                    dest_node: node_idx,
                    constraint,
                });
            }
            None => {
                tracing::warn!("VRM node[{node_idx}] has malformed VRMC_node_constraint; skipping");
            }
        }
    }

    if !entries.is_empty() {
        tracing::info!("VRM {} node constraint(s) loaded", entries.len(),);
    }

    NodeConstraintRegistry { entries }
}

fn parse_constraint(json: &Value) -> Option<NodeConstraint> {
    let obj = json.as_object()?;
    let constraint = obj.get("constraint")?.as_object()?;

    if let Some(rotation) = constraint.get("rotation") {
        let rotation_obj = rotation.as_object()?;
        let source = rotation_obj.get("source")?.as_u64()? as usize;
        let weight = rotation_obj
            .get("weight")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0) as f32;
        return Some(NodeConstraint::Rotation {
            source_node: source,
            weight,
        });
    }

    if let Some(roll) = constraint.get("roll") {
        let roll_obj = roll.as_object()?;
        let source = roll_obj.get("source")?.as_u64()? as usize;
        let roll_axis_str = roll_obj.get("rollAxis")?.as_str()?;
        let roll_axis = RollAxis::from_json_str(roll_axis_str)?;
        let weight = roll_obj
            .get("weight")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0) as f32;
        return Some(NodeConstraint::Roll {
            source_node: source,
            roll_axis,
            weight,
        });
    }

    if let Some(aim) = constraint.get("aim") {
        let aim_obj = aim.as_object()?;
        let source = aim_obj.get("source")?.as_u64()? as usize;
        let aim_axis_str = aim_obj.get("aimAxis")?.as_str()?;
        let aim_axis = AimAxis::from_json_str(aim_axis_str)?;
        let weight = aim_obj
            .get("weight")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0) as f32;
        return Some(NodeConstraint::Aim {
            source_node: source,
            aim_axis,
            weight,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_json(json: &str) -> Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parse_rotation_constraint() {
        let json = make_json(
            r#"{
                "specVersion": "1.0",
                "constraint": {
                    "rotation": {
                        "source": 5,
                        "weight": 0.8
                    }
                }
            }"#,
        );
        let c = parse_constraint(&json).unwrap();
        assert!(
            matches!(c, NodeConstraint::Rotation { .. }),
            "expected Rotation, got {c:?}"
        );
        let NodeConstraint::Rotation {
            source_node,
            weight,
        } = c
        else {
            return;
        };
        assert_eq!(source_node, 5);
        assert!((weight - 0.8).abs() < 1e-6);
    }

    #[test]
    fn parse_roll_constraint() {
        let json = make_json(
            r#"{
                "specVersion": "1.0",
                "constraint": {
                    "roll": {
                        "source": 3,
                        "rollAxis": "Y",
                        "weight": 1.0
                    }
                }
            }"#,
        );
        let c = parse_constraint(&json).unwrap();
        assert!(
            matches!(c, NodeConstraint::Roll { .. }),
            "expected Roll, got {c:?}"
        );
        let NodeConstraint::Roll {
            source_node,
            roll_axis,
            weight,
        } = c
        else {
            return;
        };
        assert_eq!(source_node, 3);
        assert_eq!(roll_axis, RollAxis::Y);
        assert!((weight - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_aim_constraint() {
        let json = make_json(
            r#"{
                "specVersion": "1.0",
                "constraint": {
                    "aim": {
                        "source": 7,
                        "aimAxis": "PositiveZ",
                        "weight": 0.5
                    }
                }
            }"#,
        );
        let c = parse_constraint(&json).unwrap();
        assert!(
            matches!(c, NodeConstraint::Aim { .. }),
            "expected Aim, got {c:?}"
        );
        let NodeConstraint::Aim {
            source_node,
            aim_axis,
            weight,
        } = c
        else {
            return;
        };
        assert_eq!(source_node, 7);
        assert_eq!(aim_axis, AimAxis::PositiveZ);
        assert!((weight - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parse_default_weight() {
        let json = make_json(
            r#"{
                "specVersion": "1.0",
                "constraint": {
                    "rotation": {
                        "source": 0
                    }
                }
            }"#,
        );
        let c = parse_constraint(&json).unwrap();
        assert!((c.weight() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_malformed_returns_none() {
        let json = make_json(r#"{"specVersion": "1.0"}"#);
        assert!(parse_constraint(&json).is_none());
    }

    #[test]
    fn parse_unknown_aim_axis_returns_none() {
        let json = make_json(
            r#"{
                "constraint": {
                    "aim": {
                        "source": 0,
                        "aimAxis": "Diagonal"
                    }
                }
            }"#,
        );
        assert!(parse_constraint(&json).is_none());
    }

    #[test]
    fn eval_rotation_identity_when_source_at_rest() {
        let src_rest = Quat::from_rotation_y(0.3);
        let dst_rest = Quat::from_rotation_x(0.5);
        let result = NodeConstraintRegistry::eval_rotation(src_rest, src_rest, dst_rest, 1.0);
        let diff = (result * dst_rest.inverse()).angle_between(Quat::IDENTITY);
        assert!(diff < 1e-5, "expected dst_rest, got {result:?}");
    }

    #[test]
    fn eval_rotation_full_weight_copies_delta() {
        let src_rest = Quat::IDENTITY;
        let src_local = Quat::from_rotation_y(0.5);
        let dst_rest = Quat::IDENTITY;
        let result = NodeConstraintRegistry::eval_rotation(src_local, src_rest, dst_rest, 1.0);
        let diff = result.angle_between(src_local);
        assert!(diff < 1e-5, "expected src_local delta, got {result:?}");
    }

    #[test]
    fn eval_rotation_zero_weight_keeps_rest() {
        let src_local = Quat::from_rotation_y(1.0);
        let src_rest = Quat::IDENTITY;
        let dst_rest = Quat::from_rotation_x(0.3);
        let result = NodeConstraintRegistry::eval_rotation(src_local, src_rest, dst_rest, 0.0);
        let diff = result.angle_between(dst_rest);
        assert!(diff < 1e-5, "expected dst_rest, got {result:?}");
    }

    #[test]
    fn eval_aim_source_at_destination_returns_rest() {
        let dst_rest = Quat::IDENTITY;
        let parent_rot = Quat::IDENTITY;
        let pos = Vec3::new(1.0, 2.0, 3.0);
        let result = NodeConstraintRegistry::eval_aim(
            pos,
            pos,
            dst_rest,
            parent_rot,
            AimAxis::PositiveZ,
            1.0,
        );
        let diff = result.angle_between(dst_rest);
        assert!(diff < 1e-5, "expected dst_rest when source == dest");
    }

    #[test]
    fn roll_axis_as_vec3() {
        assert_eq!(RollAxis::X.as_vec3(), Vec3::X);
        assert_eq!(RollAxis::Y.as_vec3(), Vec3::Y);
        assert_eq!(RollAxis::Z.as_vec3(), Vec3::Z);
    }

    #[test]
    fn aim_axis_as_vec3() {
        assert_eq!(AimAxis::PositiveX.as_vec3(), Vec3::X);
        assert_eq!(AimAxis::NegativeX.as_vec3(), -Vec3::X);
        assert_eq!(AimAxis::PositiveY.as_vec3(), Vec3::Y);
        assert_eq!(AimAxis::NegativeY.as_vec3(), -Vec3::Y);
        assert_eq!(AimAxis::PositiveZ.as_vec3(), Vec3::Z);
        assert_eq!(AimAxis::NegativeZ.as_vec3(), -Vec3::Z);
    }

    #[test]
    fn registry_is_empty_by_default() {
        let reg = NodeConstraintRegistry::default();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn constraint_source_and_weight_accessors() {
        let c = NodeConstraint::Rotation {
            source_node: 42,
            weight: 0.75,
        };
        assert_eq!(c.source_node(), 42);
        assert!((c.weight() - 0.75).abs() < 1e-6);
    }
}
