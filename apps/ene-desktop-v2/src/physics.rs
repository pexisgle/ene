use glam::Vec3;
use hecs::Entity;
use rapier3d::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub scale: f32,
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
    /// Maps hecs Entity to Rapier RigidBodyHandle
    pub entity_to_body: HashMap<Entity, RigidBodyHandle>,
    /// Maps hecs Entity to its Rapier `ColliderHandle`s. One entity
    /// can own multiple colliders — the character, for example,
    /// owns one sphere per humanoid bone.
    pub entity_to_colliders: HashMap<Entity, Vec<ColliderHandle>>,
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

    /// Cast a ray against every collider in the world. Returns
    /// `Some((entity, toi))` for the first hit (by `toi`), or
    /// `None` on a clean miss. The hit test is BVH-accelerated
    /// internally by Rapier — no extra caching on our side.
    pub fn cast_ray(
        &self,
        origin: Point<f32>,
        dir: Vector<f32>,
        max_toi: f32,
    ) -> Option<(Entity, f32)> {
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
        // Find which entity owns the hit collider. One entity can
        // own multiple colliders (the character has one sphere per
        // bone), so we scan all of them.
        for (entity, colliders) in &self.entity_to_colliders {
            if colliders.contains(&handle) {
                return Some((*entity, toi));
            }
        }
        None
    }

    /// PR5.2: add (or replace) the character's per-bone sphere
    /// colliders. Each entry is `(world_position, radius)`. All
    /// colliders share a single kinematic rigid body parked at
    /// the world origin; the per-frame
    /// [`Self::update_character_bone_positions`] only moves the
    /// colliders (not the body), which is the cheap path —
    /// Rapier doesn't rebuild any acceleration structure for a
    /// collider translation.
    ///
    /// `world_position` is the bone's location in the model's
    /// local frame (after `T(-center) * S(normalize_scale)`).
    /// The collider's local position is set to exactly that
    /// vector, so when the body sits at the origin the collider's
    /// world position equals `world_position`.
    pub fn add_character_bone_colliders(&mut self, entity: Entity, bones: &[(Vec3, f32)]) {
        // Drop any prior body / colliders for this entity so the
        // new bone set replaces the old one instead of stacking.
        self.remove_character_colliders(entity);

        let body = RigidBodyBuilder::kinematic_position_based().build();
        let body_handle = self.rigid_body_set.insert(body);

        let mut collider_handles = Vec::with_capacity(bones.len());
        for (world_pos, radius) in bones {
            let collider = ColliderBuilder::ball(*radius)
                .translation(vector![world_pos.x, world_pos.y, world_pos.z])
                .build();
            let collider_handle = self.collider_set.insert_with_parent(
                collider,
                body_handle,
                &mut self.rigid_body_set,
            );
            collider_handles.push(collider_handle);
        }

        self.entity_to_body.insert(entity, body_handle);
        self.entity_to_colliders.insert(entity, collider_handles);
    }

    /// PR5.2: move every bone collider to the matching entry in
    /// `positions` and slide the underlying body to
    /// `character_position` so the whole rig follows the
    /// character (drag, animation, anything that mutates
    /// `settings.character_state.character_position`).
    ///
    /// `positions` is the bone list in the model's local frame
    /// (after `T(-center) * S(normalize_scale)`); each entry is
    /// multiplied by `actual_scale` to land in the same world
    /// frame the per-frame model matrix produces. The body
    /// itself is then translated to `character_position` so the
    /// colliders' world positions become
    /// `character_position + position * actual_scale`, which
    /// matches the rendered mesh.
    ///
    /// Uses [`Collider::set_translation_wrt_parent`] (not
    /// [`Collider::set_translation`]) because Rapier recomputes
    /// the world position from `body * local` each step. The
    /// collider's `radius` is set once at construction time and
    /// is **not** scaled by `actual_scale` here — the user
    /// accepted a slightly loose collider, so we leave the
    /// initial radius in place. If `model_scale` drifts far from
    /// the value at construction time, rebuild the colliders.
    pub fn update_character_bone_positions(
        &mut self,
        entity: Entity,
        positions: &[Vec3],
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
        for (handle, pos) in colliders.iter().zip(positions.iter()) {
            if let Some(collider) = self.collider_set.get_mut(*handle) {
                let scaled = *pos * actual_scale;
                collider.set_translation_wrt_parent(vector![scaled.x, scaled.y, scaled.z]);
            }
        }
    }

    /// PR5.2: detach and free every collider (and the body they
    /// were attached to) for `entity`. Used by the per-character
    /// rebuild path in `add_character_bone_colliders`, and useful
    /// from tests.
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
    }
}

#[cfg(test)]
mod tests {
    //! PR5.2 regression tests: the character's per-bone sphere
    //! colliders are the source of truth for the click-through hit
    //! test. The bones are placed in the model's local frame
    //! (after `T(-center) * S(normalize_scale)`); the rigid body
    //! sits at the world origin and the colliders follow the bone
    //! positions every frame via
    //! [`PhysicsWorld::update_character_bone_positions`].
    use super::*;
    use glam::Vec3;
    use hecs::{Entity, World};

    fn setup() -> (PhysicsWorld, World, Entity) {
        let mut world = World::new();
        let entity = world.spawn(());
        let physics = PhysicsWorld::new();
        (physics, world, entity)
    }

    /// Two bones: a "chest" at (0, 0, 0) and a "head" at (0, 0.5, 0).
    /// The colliders land at those local-frame positions; the body
    /// itself stays at the world origin.
    fn two_bones() -> Vec<(Vec3, f32)> {
        vec![
            (Vec3::new(0.0, 0.0, 0.0), 0.2),
            (Vec3::new(0.0, 0.5, 0.0), 0.1),
        ]
    }

    /// `add_character_bone_colliders` must create a single body
    /// (parked at the origin) and the right number of sphere
    /// colliders at the supplied positions.
    #[test]
    fn add_character_bone_colliders_creates_body_and_spheres() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bones());

        let body_handle = physics.entity_to_body[&entity];
        let body = physics.rigid_body_set.get(body_handle).unwrap();
        let t = body.translation();
        assert!(
            (t.x.abs() + t.y.abs() + t.z.abs()) < 1e-5,
            "body must stay at the world origin so the collider local positions = world positions, got {t:?}"
        );

        let colliders = &physics.entity_to_colliders[&entity];
        assert_eq!(colliders.len(), 2, "must register one collider per bone");
        for (i, (expected_pos, expected_radius)) in two_bones().iter().enumerate() {
            let collider = physics.collider_set.get(colliders[i]).unwrap();
            let ball = collider
                .shape()
                .as_ball()
                .expect("bone collider must be a sphere");
            assert!(
                (ball.radius - expected_radius).abs() < 1e-5,
                "bone {i} radius must be {expected_radius}, got {}",
                ball.radius
            );
            let ct = collider.translation();
            assert!(
                (ct.x - expected_pos.x).abs() < 1e-5
                    && (ct.y - expected_pos.y).abs() < 1e-5
                    && (ct.z - expected_pos.z).abs() < 1e-5,
                "bone {i} collider translation must equal {expected_pos:?}, got {ct:?}"
            );
        }
    }

    /// A ray through the centre of a bone sphere must hit the
    /// character's entity. A ray past the sphere must miss.
    #[test]
    fn cast_ray_finds_bone_collider_via_entity() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bones());
        physics.step();

        // Through bone 0 (chest) at (0, 0, 0) with radius 0.2.
        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "ray through chest must hit");
        assert_eq!(hit.unwrap().0, entity, "hit must be the character entity");

        // Through bone 1 (head) at (0, 0.5, 0) with radius 0.1.
        let hit = physics.cast_ray(Point::new(0.0, 0.5, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "ray through head must hit");

        // Above the head: y = 0.7 is outside both spheres.
        let hit = physics.cast_ray(Point::new(0.0, 0.7, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_none(), "ray above head must miss all bone spheres");
    }

    /// `update_character_bone_positions` must move each collider
    /// to the new position without rebuilding any geometry, and the
    /// hit test must follow the move. Also pins the body to the
    /// supplied `character_position` and scales the collider
    /// local positions by `actual_scale` so the world-space
    /// collider matches the rendered mesh.
    #[test]
    fn update_character_bone_positions_moves_colliders() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bones());
        physics.step();

        // Move the rig +5 along X (body translation), scale by 1.5.
        // Local positions stay at the original (0, 0) / (0, 0.5) so
        // the test isolates the body-translation / scale effects.
        physics.update_character_bone_positions(
            entity,
            &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.5, 0.0)],
            Vec3::new(5.0, 0.0, 0.0),
            1.5,
        );
        physics.step();

        // A ray through the old chest position (0, 0, 0) must miss
        // — the rig is now centred at (5, 0, 0).
        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(
            hit.is_none(),
            "ray at the old chest position must miss after move"
        );

        // A ray through the new chest position (5, 0, 0) — at the
        // collider's local (0, 0, 0) plus the body translation —
        // must hit.
        let hit = physics.cast_ray(Point::new(5.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(
            hit.is_some(),
            "ray at the new chest position must hit after move"
        );

        // A ray through the new head position. The collider's
        // local (0, 0.5, 0) is scaled to (0, 0.75, 0) by actual_scale=1.5
        // and the body sits at (5, 0, 0), so the world centre is
        // (5, 0.75, 0).
        let hit = physics.cast_ray(Point::new(5.0, 0.75, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "ray at the scaled head position must hit");
    }

    /// `remove_character_colliders` must free the body and drop
    /// the entity mapping, so a subsequent cast_ray no longer hits.
    #[test]
    fn remove_character_colliders_drops_body() {
        let (mut physics, _world, entity) = setup();
        physics.add_character_bone_colliders(entity, &two_bones());
        physics.step();

        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_some(), "precondition: ray must hit before removal");

        physics.remove_character_colliders(entity);

        let hit = physics.cast_ray(Point::new(0.0, 0.0, 3.0), vector![0.0, 0.0, -1.0], 100.0);
        assert!(hit.is_none(), "ray must miss after removal");
        assert!(physics.entity_to_body.get(&entity).is_none());
        assert!(physics.entity_to_colliders.get(&entity).is_none());
    }
}
