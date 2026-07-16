//! Raycast collider debug overlay glue.
//!
//! Walks the per-bone collider set owned by the character entity
//! and pushes a [`DebugLine`] list ready for the [`DebugRenderer`]:
//!
//! - one wireframe per bone collider — a sphere for
//!   `BoneShape::Sphere` entries (head, jaw, eyes, ...) and a
//!   Y-axis capsule for `BoneShape::Capsule` / `BoneShape::CapsuleY`
//!   entries (limbs, trunk, foot). The collider's current world
//!   rotation is honoured so a swinging arm's wireframe follows the
//!   bone (cyan when idle, yellow when the bone is the raycast hit
//!   target).
//! - a 3-segment cross at the raycast hit point (red), only when
//!   the latest hit was on the character entity.
//!
//! The cross half-edge is [`CROSS_HALF_EXTENT`] from the renderer.
use rapier3d::prelude::{ColliderHandle, ColliderSet};

use crate::physics::PhysicsWorld;
use ene_vrm::debug_renderer::{
    CROSS_HALF_EXTENT, DebugLine, capsule_wireframe_lines_into, cross_lines,
    sphere_wireframe_lines_into,
};

/// Colour for colliders that the latest raycast did **not**
/// hit. Cyan at 50 % alpha — readable but not loud, so
/// the model silhouette stays the dominant visual.
pub const IDLE_COLOR: glam::Vec4 = glam::Vec4::new(0.2, 0.85, 1.0, 0.5);

/// Colour for the collider that the latest raycast did
/// hit. Yellow at 90 % alpha — loud enough to read as
/// "this is the one".
pub const HIT_COLOR: glam::Vec4 = glam::Vec4::new(1.0, 0.85, 0.2, 0.9);

/// Colour for the hit-point cross. Red at 95 % alpha.
pub const HIT_POINT_COLOR: glam::Vec4 = glam::Vec4::new(1.0, 0.3, 0.3, 0.95);

/// Build the line list for the character-window debug
/// overlay.
///
/// - For every collider in `colliders`, push a wireframe sphere
///   (or Y-axis capsule) of the collider's radius / half-height
///   at the collider's current world position, rotated to match
///   the collider's current orientation.
/// - The collider whose handle matches `hit_collider` (if any)
///   is coloured yellow; the rest are cyan.
/// - When `hit_collider` is `Some`, push a 3-axis cross at
///   `hit_point` so the precise impact location is obvious.
///
/// The function never reads the GPU or the surface — it
/// is pure CPU and is safe to call from anywhere in the
/// per-frame pipeline. The runtime is responsible for passing
/// the per-entity collider list (read from the
/// [`crate::component::physics::PhysicsColliders`] component)
/// and the latest hit from
/// [`crate::state::DebugState::last_raycast_hit`].
pub fn build_collider_lines(
    out: &mut Vec<DebugLine>,
    physics: &PhysicsWorld,
    colliders: &[ColliderHandle],
    hit_collider: Option<ColliderHandle>,
    hit_point: Option<glam::Vec3>,
    show_all: bool,
) {
    out.clear();
    push_lines(out, physics, colliders, hit_collider, show_all);
    if let Some(point) = hit_point {
        cross_lines(point, CROSS_HALF_EXTENT, HIT_POINT_COLOR, out);
    }
}

/// Pure version of [`build_collider_lines`] that doesn't draw
/// the hit cross; useful for tests and for callers that need
/// the wireframe only.
pub fn push_lines(
    out: &mut Vec<DebugLine>,
    physics: &PhysicsWorld,
    colliders: &[ColliderHandle],
    hit_collider: Option<ColliderHandle>,
    show_all: bool,
) {
    for handle in colliders {
        let Some(collider) = physics.collider_set.get(*handle) else {
            continue;
        };
        if !show_all && Some(*handle) != hit_collider {
            continue;
        }
        let position = collider.position();
        let center = glam::Vec3::new(
            position.translation.x,
            position.translation.y,
            position.translation.z,
        );
        // Convert the collider's nalgebra quaternion to a
        // `glam::Quat` so the capsule wireframe helper can
        // apply it to its local +Y. `position.rotation`
        // returns a `UnitQuaternion`; `.quaternion()`
        // exposes the raw `Quaternion` (i, j, k, w).
        let q = position.rotation;
        let orientation = q;
        let color = if Some(*handle) == hit_collider {
            HIT_COLOR
        } else {
            IDLE_COLOR
        };
        if let Some(ball) = collider.shape().as_ball() {
            sphere_wireframe_lines_into(center, ball.radius, color, out);
        } else if let Some(capsule) = collider.shape().as_capsule() {
            // Rapier stores a capsule as `(half_height, radius)`
            // along its local +Y. The collider-local rotation is
            // already baked in via `ColliderBuilder::rotation` +
            // `set_rotation_wrt_parent`, so the world transform
            // `position` is what we want here.
            capsule_wireframe_lines_into(
                center,
                capsule.half_height(),
                capsule.radius,
                orientation,
                color,
                out,
            );
        }
    }
}

#[expect(
    dead_code,
    reason = "marker retained for future collider debug integration"
)]
pub const fn unused_collider_set_marker(_cs: &ColliderSet) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::BoneShapeSpec;
    use crate::character::collider::BoneShape;
    use crate::physics::PhysicsWorld;
    use glam::{Quat, Vec3};

    fn setup() -> (PhysicsWorld, Vec<ColliderHandle>) {
        let mut physics = PhysicsWorld::new();
        let specs = vec![
            BoneShapeSpec {
                bone_node: 0,
                local_position: Vec3::ZERO,
                local_rotation: Quat::IDENTITY,
                shape: BoneShape::Sphere { radius: 0.2 },
                static_offset: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
            },
            BoneShapeSpec {
                bone_node: 0,
                local_position: Vec3::new(0.0, 0.5, 0.0),
                local_rotation: Quat::IDENTITY,
                shape: BoneShape::Sphere { radius: 0.1 },
                static_offset: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
            },
        ];
        let reg = physics.register_character_colliders(&specs);
        (physics, reg.colliders)
    }

    /// With no colliders, the output is empty (no spheres, no
    /// cross).
    #[test]
    fn empty_when_no_colliders() {
        let (physics, _) = setup();
        let mut out = Vec::new();
        build_collider_lines(&mut out, &physics, &[], None, None, true);
        assert!(out.is_empty(), "no colliders => no lines");
    }

    /// Two bone colliders must produce the expected number
    /// of lines: `2 * (longitudes² + (latitudes + 1) * longitudes)`.
    /// No hit => no cross.
    #[test]
    fn two_colliders_produces_expected_line_count() {
        let (physics, colliders) = setup();
        let mut out = Vec::new();
        build_collider_lines(&mut out, &physics, &colliders, None, None, true);
        let per_sphere = ene_vrm::SPHERE_LONGITUDES * ene_vrm::SPHERE_LONGITUDES
            + (ene_vrm::SPHERE_LATITUDES + 1) * ene_vrm::SPHERE_LONGITUDES;
        assert_eq!(out.len(), 2 * per_sphere);
    }

    /// A hit-point cross adds exactly 3 segments (one per
    /// axis) to the line list, regardless of how many
    /// colliders are registered.
    #[test]
    fn hit_point_adds_three_cross_segments() {
        let mut physics = PhysicsWorld::new();
        let specs = vec![BoneShapeSpec {
            bone_node: 0,
            local_position: Vec3::ZERO,
            local_rotation: Quat::IDENTITY,
            shape: BoneShape::Sphere { radius: 0.2 },
            static_offset: Vec3::ZERO,
            rest_rotation: Quat::IDENTITY,
        }];
        let reg = physics.register_character_colliders(&specs);
        physics.step();
        let hit_point = Vec3::new(0.0, 0.0, 0.2);
        let mut out = Vec::new();
        build_collider_lines(
            &mut out,
            &physics,
            &reg.colliders,
            None,
            Some(hit_point),
            true,
        );
        let per_sphere = ene_vrm::SPHERE_LONGITUDES * ene_vrm::SPHERE_LONGITUDES
            + (ene_vrm::SPHERE_LATITUDES + 1) * ene_vrm::SPHERE_LONGITUDES;
        assert_eq!(out.len(), per_sphere + 3);
    }

    /// A capsule collider must produce capsule wireframe
    /// lines (caps + meridians), not zero lines. Guards
    /// against future refactors that drop capsule support
    /// and leave limbs / trunk invisible in the overlay.
    #[test]
    fn capsule_collider_produces_wireframe_lines() {
        let mut physics = PhysicsWorld::new();
        let specs = vec![BoneShapeSpec {
            bone_node: 0,
            local_position: Vec3::ZERO,
            local_rotation: Quat::IDENTITY,
            shape: BoneShape::CapsuleY {
                half_height: 0.3,
                radius: 0.1,
            },
            static_offset: Vec3::ZERO,
            rest_rotation: Quat::IDENTITY,
        }];
        let reg = physics.register_character_colliders(&specs);
        physics.step();
        let mut out = Vec::new();
        build_collider_lines(&mut out, &physics, &reg.colliders, None, None, true);
        // capsule_wireframe_lines_into emits
        // 2 * SPHERE_LONGITUDES (caps) + SPHERE_LONGITUDES
        // (meridians) = 3 * SPHERE_LONGITUDES segments.
        assert_eq!(out.len(), 3 * ene_vrm::SPHERE_LONGITUDES);
    }
}
