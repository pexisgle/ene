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
    /// Maps hecs Entity to Rapier ColliderHandle
    pub entity_to_collider: HashMap<Entity, ColliderHandle>,
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
            entity_to_collider: HashMap::new(),
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

    /// Cast a ray to find the first collider it hits.
    pub fn cast_ray(
        &self,
        origin: Point<f32>,
        dir: Vector<f32>,
        max_toi: f32,
    ) -> Option<(Entity, f32)> {
        let ray = Ray::new(origin, dir);
        let filter = QueryFilter::default();

        if let Some((handle, toi)) = self.query_pipeline.cast_ray(
            &self.rigid_body_set,
            &self.collider_set,
            &ray,
            max_toi,
            true,
            filter,
        ) {
            // Find which entity this collider belongs to
            let entity = self
                .entity_to_collider
                .iter()
                .find(|&(_, &h)| h == handle)
                .map(|(&e, _)| e);
            entity.map(|e| (e, toi))
        } else {
            None
        }
    }

    /// Update the character's collider based on its loaded AABB and current transform.
    pub fn update_character_collider(
        &mut self,
        entity: Entity,
        aabb: Option<([f32; 3], [f32; 3])>,
        translation: Vec3,
        scale: f32,
    ) {
        let Some((min, max)) = aabb else {
            return;
        };

        let local_hx = (max[0] - min[0]) * 0.5 * scale;
        let local_hy = (max[1] - min[1]) * 0.5 * scale;
        let local_hz = (max[2] - min[2]) * 0.5 * scale;

        let cx = (max[0] + min[0]) * 0.5 * scale;
        let cy = (max[1] + min[1]) * 0.5 * scale;
        let cz = (max[2] + min[2]) * 0.5 * scale;

        let target_pos = vector![translation.x + cx, translation.y + cy, translation.z + cz];

        if let Some(&body_handle) = self.entity_to_body.get(&entity) {
            let body = self.rigid_body_set.get_mut(body_handle).unwrap();
            body.set_translation(target_pos, true);

            // For simplicity, we just recreate the collider if scale changes.
            // But if we assume scale is mostly constant, we can just leave it.
            // A full implementation would check if shape changed.
            if let Some(&_collider_handle) = self.entity_to_collider.get(&entity) {
                // If we needed to resize the collider dynamically we would replace it.
                // For now, just update the rigid body position.
            }
        } else {
            // Create rigid body and collider
            let rigid_body = RigidBodyBuilder::kinematic_position_based()
                .translation(target_pos)
                .build();
            let body_handle = self.rigid_body_set.insert(rigid_body);

            let collider = ColliderBuilder::cuboid(local_hx, local_hy, local_hz).build();
            let collider_handle = self.collider_set.insert_with_parent(
                collider,
                body_handle,
                &mut self.rigid_body_set,
            );

            self.entity_to_body.insert(entity, body_handle);
            self.entity_to_collider.insert(entity, collider_handle);
        }
    }
}
