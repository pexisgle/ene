//! CPU-side spring-bone collider wires for the overlay debug pass.

use glam::{Mat4, Quat, Vec3, Vec4};

use ene_vrm::debug_renderer::{
    DebugLine, capsule_wireframe_lines_into, sphere_wireframe_lines_into,
};
use ene_vrm::{NodeHierarchy, SpringBoneCollider, SpringBoneShape};

const COLLIDER_WIRE: Vec4 = Vec4::new(1.0, 0.28, 0.82, 1.0);
const MIN_RADIUS: f32 = 1.0e-6;
const MIN_CAPSULE_LEN: f32 = 1.0e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OverlayCollider {
    Sphere {
        center: Vec3,
        radius: f32,
    },
    Capsule {
        center: Vec3,
        half_height: f32,
        radius: f32,
        orientation: Quat,
    },
}

#[must_use]
pub(crate) fn overlay_collider(
    shape: &SpringBoneShape,
    node_pos: Vec3,
    node_rot: Quat,
    model: Mat4,
    uniform_scale: f32,
) -> Option<OverlayCollider> {
    if !uniform_scale.is_finite() {
        return None;
    }
    let scale = uniform_scale.abs();
    if scale <= MIN_RADIUS {
        return None;
    }
    match shape {
        SpringBoneShape::Sphere { offset, radius } => overlay_sphere(
            node_pos,
            node_rot,
            Vec3::from(*offset),
            *radius,
            model,
            scale,
        ),
        SpringBoneShape::Capsule {
            offset,
            radius,
            tail,
        } => overlay_capsule(
            node_pos,
            node_rot,
            Vec3::from(*offset),
            Vec3::from(*tail),
            *radius,
            model,
            scale,
        ),
        _ => None,
    }
}

fn overlay_sphere(
    node_pos: Vec3,
    node_rot: Quat,
    offset: Vec3,
    radius: f32,
    model: Mat4,
    scale: f32,
) -> Option<OverlayCollider> {
    let scaled_radius = radius * scale;
    if !scaled_radius.is_finite() || scaled_radius <= MIN_RADIUS {
        return None;
    }
    let local = node_pos + node_rot * offset;
    if !local.is_finite() {
        return None;
    }
    let center = model.transform_point3(local);
    if !center.is_finite() {
        return None;
    }
    Some(OverlayCollider::Sphere {
        center,
        radius: scaled_radius,
    })
}

fn overlay_capsule(
    node_pos: Vec3,
    node_rot: Quat,
    offset: Vec3,
    tail: Vec3,
    radius: f32,
    model: Mat4,
    scale: f32,
) -> Option<OverlayCollider> {
    let scaled_radius = radius * scale;
    if !scaled_radius.is_finite() || scaled_radius <= MIN_RADIUS {
        return None;
    }
    let local_start = node_pos + node_rot * offset;
    let local_end = node_pos + node_rot * tail;
    if !local_start.is_finite() || !local_end.is_finite() {
        return None;
    }
    let start = model.transform_point3(local_start);
    let end = model.transform_point3(local_end);
    if !start.is_finite() || !end.is_finite() {
        return None;
    }
    let delta = end - start;
    let len = delta.length();
    if !len.is_finite() {
        return None;
    }
    if len <= MIN_CAPSULE_LEN {
        return Some(OverlayCollider::Sphere {
            center: start,
            radius: scaled_radius,
        });
    }
    let dir = delta / len;
    if !dir.is_finite() {
        return None;
    }
    Some(OverlayCollider::Capsule {
        center: start + delta * 0.5,
        half_height: len * 0.5,
        radius: scaled_radius,
        orientation: Quat::from_rotation_arc(Vec3::Y, dir),
    })
}

fn node_world(nodes: &NodeHierarchy, index: usize) -> Option<(Vec3, Quat)> {
    let pos = nodes.world_positions.get(index).copied()?;
    let rot = nodes.world_rotations.get(index).copied()?;
    if !pos.is_finite() || !rot.is_finite() {
        return None;
    }
    Some((pos, rot))
}

fn push_overlay_collider(shape: OverlayCollider, out: &mut Vec<DebugLine>) {
    match shape {
        OverlayCollider::Sphere { center, radius } => {
            sphere_wireframe_lines_into(center, radius, COLLIDER_WIRE, out);
        }
        OverlayCollider::Capsule {
            center,
            half_height,
            radius,
            orientation,
        } => {
            capsule_wireframe_lines_into(
                center,
                half_height,
                radius,
                orientation,
                COLLIDER_WIRE,
                out,
            );
        }
    }
}

#[must_use]
pub(crate) fn collider_debug_lines(
    colliders: &[SpringBoneCollider],
    nodes: &NodeHierarchy,
    model: Mat4,
    uniform_scale: f32,
) -> Vec<DebugLine> {
    let mut out = Vec::new();
    for collider in colliders {
        let Some((pos, rot)) = node_world(nodes, collider.node) else {
            continue;
        };
        let Some(shape) = overlay_collider(&collider.shape, pos, rot, model, uniform_scale) else {
            continue;
        };
        push_overlay_collider(shape, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_vrm::SpringBoneCollider;

    fn single_node(pos: Vec3, rot: Quat) -> NodeHierarchy {
        NodeHierarchy {
            local_rotations: vec![rot],
            local_positions: vec![pos],
            rest_local_rotations: vec![rot],
            rest_local_positions: vec![pos],
            parents: vec![-1],
            world_rotations: vec![rot],
            world_positions: vec![pos],
            rest_world_rotations: vec![rot],
            rest_world_positions: vec![pos],
        }
    }

    fn sphere_of(shape: Option<OverlayCollider>) -> (Vec3, f32) {
        match shape {
            Some(OverlayCollider::Sphere { center, radius }) => (center, radius),
            other => panic!("expected sphere, got {other:?}"),
        }
    }

    fn capsule_of(shape: Option<OverlayCollider>) -> (Vec3, f32, f32, Quat) {
        match shape {
            Some(OverlayCollider::Capsule {
                center,
                half_height,
                radius,
                orientation,
            }) => (center, half_height, radius, orientation),
            other => panic!("expected capsule, got {other:?}"),
        }
    }

    #[test]
    fn sphere_follows_node_offset() {
        let (center, radius) = sphere_of(overlay_collider(
            &SpringBoneShape::Sphere {
                offset: [1.0, 0.0, 0.0],
                radius: 0.25,
            },
            Vec3::new(10.0, 2.0, 3.0),
            Quat::IDENTITY,
            Mat4::IDENTITY,
            1.0,
        ));
        assert!((center - Vec3::new(11.0, 2.0, 3.0)).length() < 1e-5);
        assert!((radius - 0.25).abs() < 1e-5);
    }

    #[test]
    fn model_scale_applies_to_center_and_radius() {
        let model = Mat4::from_scale(Vec3::splat(2.0));
        let (center, radius) = sphere_of(overlay_collider(
            &SpringBoneShape::Sphere {
                offset: [1.0, 0.0, 0.0],
                radius: 0.5,
            },
            Vec3::new(10.0, 0.0, 0.0),
            Quat::IDENTITY,
            model,
            2.0,
        ));
        assert!((center - Vec3::new(22.0, 0.0, 0.0)).length() < 1e-5);
        assert!((radius - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rotated_node_moves_offset() {
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let (center, radius) = sphere_of(overlay_collider(
            &SpringBoneShape::Sphere {
                offset: [1.0, 0.0, 0.0],
                radius: 0.1,
            },
            Vec3::ZERO,
            rot,
            Mat4::IDENTITY,
            1.0,
        ));
        assert!((center - rot * Vec3::X).length() < 1e-5);
        assert!((radius - 0.1).abs() < 1e-5);
    }

    #[test]
    fn zero_radius_is_skipped() {
        assert!(
            overlay_collider(
                &SpringBoneShape::Sphere {
                    offset: [0.0; 3],
                    radius: 0.0,
                },
                Vec3::ZERO,
                Quat::IDENTITY,
                Mat4::IDENTITY,
                1.0,
            )
            .is_none()
        );
    }

    #[test]
    fn degenerate_capsule_becomes_sphere() {
        let (center, radius) = sphere_of(overlay_collider(
            &SpringBoneShape::Capsule {
                offset: [0.0, 1.0, 0.0],
                radius: 0.2,
                tail: [0.0, 1.0, 0.0],
            },
            Vec3::ZERO,
            Quat::IDENTITY,
            Mat4::IDENTITY,
            1.0,
        ));
        assert!((center - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
        assert!((radius - 0.2).abs() < 1e-5);
    }

    #[test]
    fn capsule_axis_matches_tail() {
        let (center, half_height, radius, orientation) = capsule_of(overlay_collider(
            &SpringBoneShape::Capsule {
                offset: [0.0, 1.0, 0.0],
                radius: 0.1,
                tail: [0.0, -1.0, 0.0],
            },
            Vec3::ZERO,
            Quat::IDENTITY,
            Mat4::IDENTITY,
            1.0,
        ));
        assert!(center.length() < 1e-5);
        assert!((half_height - 1.0).abs() < 1e-5);
        assert!((radius - 0.1).abs() < 1e-5);
        let axis = orientation * Vec3::Y;
        assert!((axis - Vec3::NEG_Y).length() < 1e-4);
    }

    #[test]
    fn missing_node_emits_no_lines() {
        let nodes = single_node(Vec3::ZERO, Quat::IDENTITY);
        let colliders = [SpringBoneCollider {
            node: 9,
            shape: SpringBoneShape::Sphere {
                offset: [0.0; 3],
                radius: 0.2,
            },
        }];
        let lines = collider_debug_lines(&colliders, &nodes, Mat4::IDENTITY, 1.0);
        assert!(lines.is_empty());
    }

    #[test]
    fn known_sphere_emits_wire_lines() {
        let nodes = single_node(Vec3::ZERO, Quat::IDENTITY);
        let colliders = [SpringBoneCollider {
            node: 0,
            shape: SpringBoneShape::Sphere {
                offset: [0.0; 3],
                radius: 0.2,
            },
        }];
        let lines = collider_debug_lines(&colliders, &nodes, Mat4::IDENTITY, 1.0);
        assert!(!lines.is_empty());
        for line in &lines {
            for point in [line.a, line.b] {
                assert!((point.length() - 0.2).abs() < 1e-3);
            }
        }
    }
}
