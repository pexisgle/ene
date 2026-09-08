//! Rapier 3D physics wrapper.
//!
//! [`PhysicsWorld`] is a thin façade over Rapier's broad phase +
//! integration pipeline. It owns the body / collider sets and
//! exposes the per-frame `step`, raycast, and bone-pose update
//! operations used by the runtime's per-frame tick.
//!
//! ## ECS integration
//!
//! The bevy `Component`s in [`crate::component::physics`] carry the
//! mapping from the character entity to Rapier handles:
//!
//! - [`crate::component::physics::PhysicsBody`] for the
//!   `RigidBodyHandle`.
//! - [`crate::component::physics::PhysicsColliders`] for the
//!   per-bone `ColliderHandle` list.
//! - [`crate::component::physics::PhysicsColliderStaticOffsets`] /
//!   [`crate::component::physics::PhysicsColliderStaticRotations`] /
//!   [`crate::component::physics::PhysicsColliderRestRotations`]
//!   for the per-bone metadata.
//!
//! [`register_character_colliders`](PhysicsWorld::register_character_colliders)
//! returns a [`CharacterColliderRegistration`] that the bevy plugin
//! stores directly on the entity as those components.
#![expect(
    dead_code,
    reason = "physics types are consumed by character registration systems"
)]
use glam::{Quat, Vec3};
use rapier3d::prelude::*;

use crate::character::{BonePose, BoneShapeSpec};

/// Kept while the ECS migration is incomplete.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub scale: f32,
}

/// One hit of a [`PhysicsWorld::cast_ray`].
#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    /// Distance along the ray, in world units.
    pub toi: f32,
    /// World-space point of the hit (`origin + dir * toi`).
    pub point: Vec3,
    /// The caller can resolve this to a bevy `Entity` via
    /// `Query<(Entity, &PhysicsColliders)>`.
    pub collider: ColliderHandle,
}

/// Result of [`PhysicsWorld::register_character_colliders`]: the
/// Rapier body and per-bone colliders plus the static metadata
/// needed by [`PhysicsWorld::update_character_bone_positions`].
///
/// Each field maps directly to a bevy `Component` in
/// [`crate::component::physics`].
#[derive(Clone, Debug)]
pub struct CharacterColliderRegistration {
    pub body: RigidBodyHandle,
    pub colliders: Vec<ColliderHandle>,
    pub static_offsets: Vec<Vec3>,
    pub static_rotations: Vec<Quat>,
    pub rest_rotations: Vec<Quat>,
}

pub struct PhysicsWorld {
    pub gravity: Vec3,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub island_manager: IslandManager,
    pub broad_phase: BroadPhaseBvh,
    pub narrow_phase: NarrowPhase,
    pub rigid_body_set: RigidBodySet,
    pub collider_set: ColliderSet,
    pub impulse_joint_set: ImpulseJointSet,
    pub multibody_joint_set: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
}

impl PhysicsWorld {
    pub fn new() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            integration_parameters: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
        }
    }

    pub fn step(&mut self) {
        self.physics_pipeline.step(
            self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            &(),
            &(),
        );
    }

    /// Cast a ray against every collider. Returns the closest hit
    /// as a [`RayHit`], or `None` on a clean miss. BVH-accelerated
    /// by Rapier; `dir` need not be normalised.
    pub fn cast_ray(&self, origin: Vec3, dir: Vec3, max_toi: f32) -> Option<RayHit> {
        let ray = Ray::new(origin, dir);
        let filter = QueryFilter::default();
        let dispatcher = self.narrow_phase.query_dispatcher();
        let pipeline = self.broad_phase.as_query_pipeline(
            dispatcher,
            &self.rigid_body_set,
            &self.collider_set,
            filter,
        );
        let (handle, toi) = pipeline.cast_ray(&ray, max_toi, true)?;
        let point = origin + dir * toi;
        Some(RayHit {
            toi,
            point,
            collider: handle,
        })
    }

    /// Build a Rapier collider for every entry in `specs` and attach
    /// them to a single kinematic body at the world origin. Returns
    /// the new body handle, the per-bone collider handles, and the
    /// per-bone metadata that
    /// [`update_character_bone_positions`](Self::update_character_bone_positions)
    /// needs each frame. The caller is responsible for storing
    /// these on the entity as `Component`s
    /// (see [`crate::component::physics`]).
    pub fn register_character_colliders(
        &mut self,
        specs: &[BoneShapeSpec],
    ) -> CharacterColliderRegistration {
        let body = RigidBodyBuilder::kinematic_position_based().build();
        let body_handle = self.rigid_body_set.insert(body);

        let mut colliders = Vec::with_capacity(specs.len());
        let mut static_offsets = Vec::with_capacity(specs.len());
        let mut static_rotations = Vec::with_capacity(specs.len());
        let mut rest_rotations = Vec::with_capacity(specs.len());
        for spec in specs {
            let builder = build_collider_for_shape(spec);
            let collider_handle = self.collider_set.insert_with_parent(
                builder,
                body_handle,
                &mut self.rigid_body_set,
            );
            colliders.push(collider_handle);
            static_offsets.push(spec.static_offset);
            static_rotations.push(spec.local_rotation);
            rest_rotations.push(spec.rest_rotation);
        }

        CharacterColliderRegistration {
            body: body_handle,
            colliders,
            static_offsets,
            static_rotations,
            rest_rotations,
        }
    }

    /// Move every bone collider to the matching entry in `poses` and
    /// slide the underlying body to `character_position`.
    ///
    /// `poses[i].translation` is in the model's local frame; it is
    /// multiplied by `actual_scale` to land in the world frame the
    /// per-frame model matrix produces. `poses[i].rotation` is the
    /// bone's current world rotation — for limbs this lets the
    /// capsule swing with the animation, for the trunk the rotation
    /// is mostly identity and the update is a no-op.
    ///
    /// Uses the `*_wrt_parent` variants because Rapier recomputes
    /// the world position from `body * local` each step. Shape
    /// dimensions are set once at construction; if `model_scale`
    /// drifts far from that value, rebuild the colliders.
    pub fn update_character_bone_positions(
        &mut self,
        reg: &CharacterColliderRegistration,
        poses: &[BonePose],
        character_position: Vec3,
        actual_scale: f32,
    ) {
        if let Some(body) = self.rigid_body_set.get_mut(reg.body) {
            body.set_translation(
                Vec3::new(
                    character_position.x,
                    character_position.y,
                    character_position.z,
                ),
                true,
            );
        }
        for (i, handle) in reg.colliders.iter().enumerate() {
            if let Some(collider) = self.collider_set.get_mut(*handle) {
                let pose = &poses[i];
                let offset = reg.static_offsets.get(i).copied().unwrap_or(Vec3::ZERO);
                let r_align = reg
                    .static_rotations
                    .get(i)
                    .copied()
                    .unwrap_or(Quat::IDENTITY);
                let rest_rot = reg.rest_rotations.get(i).copied().unwrap_or(Quat::IDENTITY);

                let r_delta = pose.rotation * rest_rot.inverse();
                let scaled_pos = pose.translation * actual_scale + r_delta * offset;
                collider.set_translation_wrt_parent(Vec3::new(
                    scaled_pos.x,
                    scaled_pos.y,
                    scaled_pos.z,
                ));

                let final_rotation = r_delta * r_align;
                let (axis, angle) = final_rotation.to_axis_angle();
                let ang = Vec3::new(axis.x * angle, axis.y * angle, axis.z * angle);
                collider.set_rotation_wrt_parent(ang);
            }
        }
    }

    /// Detach and free every collider (and the body they were
    /// attached to) listed in `reg`.
    pub fn remove_character_colliders(&mut self, reg: &CharacterColliderRegistration) {
        self.rigid_body_set.remove(
            reg.body,
            &mut self.island_manager,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            true,
        );
    }
}

fn build_collider_for_shape(spec: &BoneShapeSpec) -> ColliderBuilder {
    let mut builder = match spec.shape {
        crate::character::collider::BoneShape::Sphere { radius } => ColliderBuilder::ball(radius),
        crate::character::collider::BoneShape::CapsuleY {
            half_height,
            radius,
        }
        | crate::character::collider::BoneShape::Capsule {
            half_height,
            radius,
        } => ColliderBuilder::capsule_y(half_height, radius),
    };
    builder = builder.translation(Vec3::new(
        spec.local_position.x,
        spec.local_position.y,
        spec.local_position.z,
    ));
    // Static local rotation: for limbs this is the rest-pose
    // "toward-child" direction; `update_character_bone_positions`
    // adds the bone's animated rotation on top via
    // `set_rotation_wrt_parent`, so the capsule follows the swing.
    let (axis, angle) = spec.local_rotation.to_axis_angle();
    builder = builder.rotation(Vec3::new(axis.x * angle, axis.y * angle, axis.z * angle));
    builder
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::collider::BoneShape;
    use glam::Vec3;

    fn setup() -> PhysicsWorld {
        PhysicsWorld::new()
    }

    fn two_bone_specs() -> Vec<BoneShapeSpec> {
        vec![
            BoneShapeSpec {
                bone_node: 0,
                local_position: Vec3::new(0.0, 0.0, 0.0),
                local_rotation: Quat::IDENTITY,
                shape: BoneShape::Sphere { radius: 0.2 },
                static_offset: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
            },
            BoneShapeSpec {
                bone_node: 1,
                local_position: Vec3::new(0.0, 0.5, 0.0),
                local_rotation: Quat::IDENTITY,
                shape: BoneShape::Sphere { radius: 0.1 },
                static_offset: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
            },
        ]
    }

    fn two_bone_poses() -> Vec<BonePose> {
        vec![
            BonePose {
                translation: Vec3::new(0.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
            },
            BonePose {
                translation: Vec3::new(0.0, 0.5, 0.0),
                rotation: Quat::IDENTITY,
            },
        ]
    }

    #[test]
    fn register_character_colliders_creates_body_and_mixed_shapes() {
        let mut physics = setup();
        let specs = vec![
            BoneShapeSpec {
                bone_node: 0,
                local_position: Vec3::new(0.0, 0.0, 0.0),
                local_rotation: Quat::IDENTITY,
                shape: BoneShape::Sphere { radius: 0.2 },
                static_offset: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
            },
            BoneShapeSpec {
                bone_node: 1,
                local_position: Vec3::new(0.0, 0.5, 0.0),
                local_rotation: Quat::IDENTITY,
                shape: BoneShape::CapsuleY {
                    half_height: 0.1,
                    radius: 0.1,
                },
                static_offset: Vec3::ZERO,
                rest_rotation: Quat::IDENTITY,
            },
        ];
        let reg = physics.register_character_colliders(&specs);

        let body = physics.rigid_body_set.get(reg.body).unwrap();
        let t = body.translation();
        assert!(
            (t.x.abs() + t.y.abs() + t.z.abs()) < 1e-5,
            "body must stay at the world origin so the collider local positions = world positions, got {t:?}"
        );

        assert_eq!(
            reg.colliders.len(),
            2,
            "must register one collider per spec"
        );
        let first = physics.collider_set.get(reg.colliders[0]).unwrap();
        assert!(first.shape().as_ball().is_some(), "first spec is a sphere");
        let second = physics.collider_set.get(reg.colliders[1]).unwrap();
        assert!(
            second.shape().as_capsule().is_some(),
            "second spec is a capsule"
        );
    }

    /// A capsule whose local rotation is the bone's "toward-child"
    /// direction must be baked in so the capsule's axis points
    /// along the limb rather than world up.
    #[test]
    fn register_character_colliders_bakes_local_rotation() {
        let mut physics = setup();
        let rotation = Quat::from_rotation_arc(Vec3::Y, Vec3::X);
        let specs = vec![BoneShapeSpec {
            bone_node: 0,
            local_position: Vec3::new(0.0, 0.0, 0.0),
            local_rotation: rotation,
            shape: BoneShape::Capsule {
                half_height: 0.2,
                radius: 0.05,
            },
            static_offset: Vec3::ZERO,
            rest_rotation: Quat::IDENTITY,
        }];
        let reg = physics.register_character_colliders(&specs);
        let collider = physics.collider_set.get(reg.colliders[0]).unwrap();
        // The collider's local rotation must map the capsule's
        // local +Y (its long axis) to the same world direction
        // the spec's rotation does.
        let collider_rot = collider.rotation();
        let rapier_y: glam::Vec3 = collider_rot * glam::Vec3::new(0.0, 1.0, 0.0);
        let spec_y = rotation * glam::Vec3::Y;
        let diff = (rapier_y - spec_y).length();
        assert!(
            diff < 1e-4,
            "collider's rotated +Y must match the spec's rotated +Y; rapier={rapier_y:?}, spec={spec_y:?}, diff={diff}"
        );
    }

    #[test]
    fn cast_ray_finds_bone_collider() {
        let mut physics = setup();
        let reg = physics.register_character_colliders(&two_bone_specs());
        physics.step();

        let hit = physics.cast_ray(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(hit.is_some(), "ray through chest must hit");
        assert!(
            reg.colliders.contains(&hit.unwrap().collider),
            "hit collider must belong to the registered character"
        );

        let hit = physics.cast_ray(Vec3::new(0.0, 0.5, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(hit.is_some(), "ray through head must hit");

        let hit = physics.cast_ray(Vec3::new(0.0, 0.7, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(hit.is_none(), "ray above head must miss all bone colliders");
    }

    #[test]
    fn cast_ray_returns_point_and_collider() {
        let mut physics = setup();
        let reg = physics.register_character_colliders(&two_bone_specs());
        physics.step();

        let hit = physics
            .cast_ray(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0)
            .expect("ray through chest must hit");
        assert!(
            (hit.toi - 2.8).abs() < 1e-4,
            "toi = {}, expected 2.8",
            hit.toi
        );
        assert!(
            (hit.point.x).abs() < 1e-5
                && (hit.point.y).abs() < 1e-5
                && (hit.point.z - 0.2).abs() < 1e-5,
            "hit point must sit on the +Z side of the chest sphere, got {:?}",
            hit.point
        );
        assert!(
            reg.colliders.contains(&hit.collider),
            "the returned collider handle must belong to the character's collider set"
        );
    }

    /// `update_character_bone_positions` must move each collider
    /// to the new pose without rebuilding any geometry, and the
    /// hit test must follow the move. Also pins the body to the
    /// supplied `character_position` and scales the collider
    /// local positions by `actual_scale` so the world-space
    /// collider matches the rendered mesh.
    #[test]
    fn update_character_bone_positions_moves_colliders() {
        let mut physics = setup();
        let reg = physics.register_character_colliders(&two_bone_specs());
        physics.step();

        physics.update_character_bone_positions(
            &reg,
            &two_bone_poses(),
            Vec3::new(5.0, 0.0, 0.0),
            1.5,
        );
        physics.step();

        let hit = physics.cast_ray(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(
            hit.is_none(),
            "ray at the old chest position must miss after move"
        );

        let hit = physics.cast_ray(Vec3::new(5.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(
            hit.is_some(),
            "ray at the new chest position must hit after move"
        );

        let hit = physics.cast_ray(Vec3::new(5.0, 0.75, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(hit.is_some(), "ray at the scaled head position must hit");
    }

    /// `update_character_bone_positions` must also rotate each
    /// collider to match `pose.rotation` — without this, a
    /// swinging arm's collider stays aligned with the rest pose
    /// and clicks land on the wrong body part.
    #[test]
    fn update_character_bone_positions_rotates_colliders() {
        let mut physics = setup();
        let specs = vec![BoneShapeSpec {
            bone_node: 0,
            local_position: Vec3::ZERO,
            local_rotation: Quat::IDENTITY,
            shape: BoneShape::Capsule {
                half_height: 0.5,
                radius: 0.05,
            },
            static_offset: Vec3::ZERO,
            rest_rotation: Quat::IDENTITY,
        }];
        let reg = physics.register_character_colliders(&specs);
        physics.step();

        let pre_hit = physics.cast_ray(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(pre_hit.is_some(), "ray must hit the rest capsule");

        let poses = vec![BonePose {
            translation: Vec3::ZERO,
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        }];
        physics.update_character_bone_positions(&reg, &poses, Vec3::ZERO, 1.0);
        physics.step();

        let x_hit = physics.cast_ray(Vec3::new(0.5, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(
            x_hit.is_some(),
            "after rotating 90° about Z, a ray at (0.5, 0, 3) must hit the capsule along world X"
        );
    }

    #[test]
    fn remove_character_colliders_drops_body() {
        let mut physics = setup();
        let reg = physics.register_character_colliders(&two_bone_specs());
        physics.step();

        let hit = physics.cast_ray(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(hit.is_some(), "precondition: ray must hit before removal");

        physics.remove_character_colliders(&reg);

        let hit = physics.cast_ray(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 100.0);
        assert!(hit.is_none(), "ray must miss after removal");
    }
}
