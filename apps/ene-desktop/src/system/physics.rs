//! Bridges [`crate::component::character::BoneColliders`] and the
//! per-frame bone poses into the Rapier state owned by
//! [`crate::resource::physics::PhysicsWorldResource`].

use bevy_ecs::prelude::{Commands, Entity, Query, ResMut, Without};

use crate::component::character::BoneColliders;
use crate::component::physics::{
    PhysicsBody, PhysicsColliderRestRotations, PhysicsColliderStaticOffsets,
    PhysicsColliderStaticRotations, PhysicsColliders,
};
use crate::resource::physics::PhysicsWorldResource;

pub fn attach_bone_colliders_system(
    mut physics: ResMut<PhysicsWorldResource>,
    query: Query<(Entity, &BoneColliders), Without<PhysicsBody>>,
    mut commands: Commands,
) {
    for (entity, colliders) in &query {
        if colliders.0.is_empty() {
            // Mark the entity as processed so we don't re-scan it.
            commands
                .entity(entity)
                .insert(PhysicsBody(rapier3d::prelude::RigidBodyHandle::invalid()));
            continue;
        }
        let registered = physics.world.register_character_colliders(&colliders.0);
        commands.entity(entity).insert((
            PhysicsBody(registered.body),
            PhysicsColliders(registered.colliders),
            PhysicsColliderStaticOffsets(registered.static_offsets),
            PhysicsColliderStaticRotations(registered.static_rotations),
            PhysicsColliderRestRotations(registered.rest_rotations),
        ));
    }
}

pub fn step_physics_system(mut physics: ResMut<PhysicsWorldResource>) {
    physics.world.step();
}
