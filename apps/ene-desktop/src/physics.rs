#![allow(dead_code)]
use glam::{Quat, Vec3};
use hecs::Entity;
use rapier3d::prelude::*;
use std::collections::HashMap;

use crate::character::collider::BoneShape;
use crate::character::{BonePose, BoneShapeSpec};

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub scale: f32,
}

/// One hit of a [`PhysicsWorld::cast_ray`].
#[derive(Clone, Copy, Debug)]
pub struct RaycastHit {
    /// The hecs `Entity` that owns the hit collider.
    pub entity: Entity,
    /// Distance along the ray, in world units.
    pub toi: f32,
    /// World-space point of the hit (`origin + dir * toi`).
    pub point: Point<f32>,
    /// The Rapier `ColliderHandle` that was hit.
    pub collider: ColliderHandle,
}

/// Wrapper for Rapier3D state.
pub struct PhysicsWorld {
    pub gravity: Vector<f32>,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub island_manager: IslandManager,
    pub broad_phase: BroadPhaseMultiSap,
    pub narrow_phase: NarrowPhase,
    pub rigid_body_set: RigidBodySet,
    pub collider_set: ColliderSet,
    pub impulse_joint_set: ImpulseJointSet,
    pub multibody_joint_set: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
    pub query_pipeline: QueryPipeline,
    /// Maps hecs Entity to Rapier RigidBodyHandle.
    pub entity_to_body: HashMap<Entity, RigidBodyHandle>,
    /// Maps hecs Entity to its Rapier `ColliderHandle`s (one per bone).
    pub entity_to_colliders: HashMap<Entity, Vec<ColliderHandle>>,
    pub entity_to_collider_static_offsets: HashMap<Entity, Vec<Vec3>>,
    pub entity_to_collider_static_rotations: HashMap<Entity, Vec<Quat>>,
    pub entity_to_collider_rest_rotations: HashMap<Entity, Vec<Quat>>,
}

impl PhysicsWorld {
    pub fn new() -> Self {
        Self {
            gravity: vector![0.0, -9.81, 0.0],
            integration_parameters: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: BroadPhaseMultiSap::new(),
            narrow_phase: NarrowPhase::new(),
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            entity_to_body: HashMap::new(),
            entity_to_colliders: HashMap::new(),
            entity_to_collider_static_offsets: HashMap::new(),
            entity_to_collider_static_rotations: HashMap::new(),
            entity_to_collider_rest_rotations: HashMap::new(),
        }
    }

    pub fn step(&mut self) {
        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            None,
            &(),
            &(),
        );

        self.query_pipeline.update(&self.collider_set);
    }

    /// Cast a ray against every collider. Returns the closest hit
    /// as a [`RaycastHit`], or `None` on a clean miss. BVH-accelerated
    /// by Rapier; `dir` need not be normalised.
    pub fn cast_ray(
        &self,
        origin: Point<f32>,
        dir: Vector<f32>,
        max_toi: f32,
    ) -> Option<RaycastHit> {
        let ray = Ray::new(origin, dir);
        let filter = QueryFilter::default();
        let (handle, toi) = self.query_pipeline.cast_ray(
            &self.rigid_body_set,
            &self.collider_set,
            &ray,
            max_toi,
            true,
            filter,
        )?;
        // One entity can own multiple colliders (one per bone), so
        // we scan all of them to find the owning entity.
        let entity = self
            .entity_to_colliders
            .iter()
            .find_map(|(entity, colliders)| colliders.contains(&handle).then_some(*entity))?;
        let point = Point::new(
            origin.x + dir.x * toi,
            origin.y + dir.y * toi,
            origin.z + dir.z * toi,
        );
        Some(RaycastHit {
            entity,
            toi,
            point,
            collider: handle,
        })
    }

    /// Return the collider handles owned by `entity`.
    pub fn colliders_for(&self, entity: Entity) -> &[ColliderHandle] {
        self.entity_to_colliders
            .get(&entity)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Build a Rapier collider for every entry in `specs` and attach
    /// them to a single kinematic body at the world origin. Per-frame
    /// [`Self::update_character_bone_positions`] moves each collider
    /// to follow the rendered mesh.
    pub fn add_character_bone_colliders(&mut self, entity: Entity, specs: &[BoneShapeSpec]) {
        self.remove_character_colliders(entity);

        let body = RigidBodyBuilder::kinematic_position_based().build();
        let body_handle = self.rigid_body_set.insert(body);

        let mut collider_handles = Vec::with_capacity(specs.len());
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
            collider_handles.push(collider_handle);
            static_offsets.push(spec.static_offset);
            static_rotations.push(spec.local_rotation);
            rest_rotations.push(spec.rest_rotation);
        }

        self.entity_to_body.insert(entity, body_handle);
        self.entity_to_colliders.insert(entity, collider_handles);
        self.entity_to_collider_static_offsets
            .insert(entity, static_offsets);
        self.entity_to_collider_static_rotations
            .insert(entity, static_rotations);
        self.entity_to_collider_rest_rotations
            .insert(entity, rest_rotations);
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
        entity: Entity,
        poses: &[BonePose],
        character_position: Vec3,
        actual_scale: f32,
    ) {
        if let Some(&body_handle) = self.entity_to_body.get(&entity)
            && let Some(body) = self.rigid_body_set.get_mut(body_handle)
        {
            body.set_translation(
                vector![
                    character_position.x,
                    character_position.y,
                    character_position.z
                ],
                true,
            );
        }
        let Some(colliders) = self.entity_to_colliders.get(&entity) else {
            return;
        };
        let static_offsets = self.entity_to_collider_static_offsets.get(&entity);
        let static_rotations = self.entity_to_collider_static_rotations.get(&entity);
        let rest_rotations = self.entity_to_collider_rest_rotations.get(&entity);

        for (i, handle) in colliders.iter().enumerate() {
            if let Some(collider) = self.collider_set.get_mut(*handle) {
                let pose = &poses[i];
                let offset = static_offsets.map(|v| v[i]).unwrap_or(Vec3::ZERO);
                let r_align = static_rotations.map(|v| v[i]).unwrap_or(Quat::IDENTITY);
                let rest_rot = rest_rotations.map(|v| v[i]).unwrap_or(Quat::IDENTITY);

                let r_delta = pose.rotation * rest_rot.inverse();
                let scaled_pos = pose.translation * actual_scale + r_delta * offset;
                collider.set_translation_wrt_parent(vector![
                    scaled_pos.x,
                    scaled_pos.y,
                    scaled_pos.z
                ]);

                let final_rotation = r_delta * r_align;
                let (axis, angle) = final_rotation.to_axis_angle();
                let ang = vector![axis.x * angle, axis.y * angle, axis.z * angle];
                collider.set_rotation_wrt_parent(ang);
            }
        }
    }

    /// Detach and free every collider (and the body they were
    /// attached to) for `entity`.
    pub fn remove_character_colliders(&mut self, entity: Entity) {
        if let Some(body_handle) = self.entity_to_body.remove(&entity) {
            self.rigid_body_set.remove(
                body_handle,
                &mut self.island_manager,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                true,
            );
        }
        self.entity_to_colliders.remove(&entity);
        self.entity_to_collider_static_offsets.remove(&entity);
        self.entity_to_collider_static_rotations.remove(&entity);
        self.entity_to_collider_rest_rotations.remove(&entity);
    }
}

/// Build the Rapier [`ColliderBuilder`] for one [`BoneShapeSpec`].
fn build_collider_for_shape(spec: &BoneShapeSpec) -> ColliderBuilder {
    let mut builder = match spec.shape {
        BoneShape::Sphere { radius } => ColliderBuilder::ball(radius),
        BoneShape::CapsuleY {
            half_height,
            radius,
        } => ColliderBuilder::capsule_y(half_height, radius),
        BoneShape::Capsule {
            half_height,
            radius,
        } => ColliderBuilder::capsule_y(half_height, radius),
    };
    builder = builder.translation(vector![
        spec.local_position.x,
        spec.local_position.y,
        spec.local_position.z
    ]);
    // Static local rotation: for limbs this is the rest-pose
    // "toward-child" direction; `update_character_bone_positions`
    // adds the bone's animated rotation on top via
    // `set_rotation_wrt_parent`, so the capsule follows the swing.
    let (axis, angle) = spec.local_rotation.to_axis_angle();
    builder = builder.rotation(vector![axis.x * angle, axis.y * angle, axis.z * angle]);
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use hecs::{Entity, World};

    fn setup() -> (PhysicsWorld, World, Entity) {
        let mut world = World::new();
        let entity = world.spawn(());
        let physics = PhysicsWorld::new();
        (physics, world, entity)
    }

    /// Two bone specs: a "chest" sphere at the origin and a
    /// "head" sphere at (0, 0.5, 0).
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
    fn add_character_bone_colliders_creates_body_and_mixed_shapes() {
        let (mut physics, _world, entity) = setup();
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
        physics.add_character_bone_colliders(entity, &specs);

        let body_handle = physics.entity_to_body[&entity];
        let body = physics.rigid_body_set.get(body_handle).unwrap();
        let t = body.translation();
        assert!(
            (t.x.abs() + t.y.abs() + t.z.abs()) < 1e-5,
            "body must stay at the world origin so the collider local positions = world positions, got {t:?}"
        );

        let colliders = &physics.entity_to_colliders[&entity];
        assert_eq!(colliders.len(), 2, "must register one collider per spec");
        let first = physics.collider_set.get(colliders[0]).unwrap();
        assert!(first.shape().as_ball().is_some(), "first spec is a sphere");
        let second = physics.collider_set.get(colliders[1]).unwrap();
        assert!(
            second.shape().as_capsule().is_some(),
            "second spec is a capsule"
        );
    }

    /// A capsule whose local rotation is the bone's "toward-child"
    /// direction must be baked in so the capsule's axis points
    /// along the limb rather than world up.
    #[test]
    fn add_character_bone_colliders_bakes_local_rotation() {
        let (mut physics, _world, entity) = setup();
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
        physics.add_character_bone_colliders(entity, &specs);
        let collider = physics
            .collider_set
            .get(physics.entity_to_colliders[&entity][0])
            .unwrap();
        // The collider's local rotation must map the capsule's
        // local +Y (its long axis) to the same world direction
        // the spec's rotation does.
        let collider_rot = collider.rotation();
        let rapier_y: glam::Vec3 = {
            let v = collider_rot * nalgebra::Vector3::new(0.0, 1.0, 0.0);
            glam::Vec3::new(v.x, v.y, v.z)
        };
        let spec_y = rotation * glam::Vec3::Y;
        let diff = (rapier_y - spec_y).length();
        assert!(
            diff < 1e-4,
            "collider's rotated +Y must match the spec's rotated +Y; rapier={rapier_y:?}, spec={spec_y:?}, diff={diff}"
        );
    }

    /// A ray through the centre of a bone collider must hit the
    /// character's entity. A ray past the collider must miss.
    #[test]
    fn cast_ray_finds_bone_collider_via_entity() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bone_specs());
        physics.step();

        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "ray through chest must hit");
        assert_eq!(
            hit.unwrap().entity,
            entity,
            "hit must be the character entity"
        );

        let hit = physics.cast_ray(Point::new(0.0, 0.5, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "ray through head must hit");

        let hit = physics.cast_ray(Point::new(0.0, 0.7, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_none(), "ray above head must miss all bone colliders");
    }

    /// `cast_ray` must also return the world-space hit point
    /// (`origin + dir * toi`) and the collider handle so the
    /// debug overlay can highlight the exact collider that was
    /// hit.
    #[test]
    fn cast_ray_returns_point_and_collider() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bone_specs());
        physics.step();

        let hit = physics
            .cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0)
            .expect("ray through chest must hit");
        assert_eq!(hit.entity, entity);
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
        let colliders = physics.colliders_for(entity);
        assert!(
            colliders.contains(&hit.collider),
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
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bone_specs());
        physics.step();

        physics.update_character_bone_positions(
            entity,
            &two_bone_poses(),
            Vec3::new(5.0, 0.0, 0.0),
            1.5,
        );
        physics.step();

        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(
            hit.is_none(),
            "ray at the old chest position must miss after move"
        );

        let hit = physics.cast_ray(Point::new(5.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(
            hit.is_some(),
            "ray at the new chest position must hit after move"
        );

        let hit = physics.cast_ray(Point::new(5.0, 0.75, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "ray at the scaled head position must hit");
    }

    /// `update_character_bone_positions` must also rotate each
    /// collider to match `pose.rotation` — without this, a
    /// swinging arm's collider stays aligned with the rest pose
    /// and clicks land on the wrong body part.
    #[test]
    fn update_character_bone_positions_rotates_colliders() {
        let (mut physics, _world, entity) = setup();
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
        physics.add_character_bone_colliders(entity, &specs);
        physics.step();

        let pre_hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(pre_hit.is_some(), "ray must hit the rest capsule");

        let poses = vec![BonePose {
            translation: Vec3::ZERO,
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        }];
        physics.update_character_bone_positions(entity, &poses, Vec3::ZERO, 1.0);
        physics.step();

        let x_hit = physics.cast_ray(Point::new(0.5, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(
            x_hit.is_some(),
            "after rotating 90° about Z, a ray at (0.5, 0, 3) must hit the capsule along world X"
        );
    }

    /// `remove_character_colliders` must free the body and drop
    /// the entity mapping, so a subsequent cast_ray no longer
    /// hits.
    #[test]
    fn remove_character_colliders_drops_body() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bone_specs());
        physics.step();

        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "precondition: ray must hit before removal");

        physics.remove_character_colliders(entity);

        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_none(), "ray must miss after removal");
        assert!(!physics.entity_to_body.contains_key(&entity));
        assert!(!physics.entity_to_colliders.contains_key(&entity));
    }

    #[test]
    fn colliders_for_returns_registered_handles() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bone_specs());

        let colliders = physics.colliders_for(entity);
        assert_eq!(colliders.len(), 2, "must return one handle per bone");

        let unknown = physics.colliders_for(hecs::Entity::DANGLING);
        assert!(unknown.is_empty(), "unknown entity returns an empty slice");
    }

    #[test]
    fn colliders_for_empty_after_removal() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bone_specs());
        assert_eq!(physics.colliders_for(entity).len(), 2);

        physics.remove_character_colliders(entity);
        assert!(physics.colliders_for(entity).is_empty());
    }
}
