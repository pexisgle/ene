//! Physics resources.
//!
//! [`PhysicsWorldResource`] wraps the `PhysicsWorld` so it
//! can live in the bevy world. The `Physics` plugin accesses it as
//! `Res<PhysicsWorldResource>` / `ResMut<PhysicsWorldResource>`
//! and the runtime reads it through the bevy `World`.

use bevy_ecs::prelude::Resource;

use crate::physics::PhysicsWorld;

/// The single Rapier physics world, wrapped as a bevy resource.
#[derive(Resource, Default)]
pub struct PhysicsWorldResource {
    /// The Rapier state. Public so the runtime in
    /// [`crate::runtime`] can read it without going through a
    /// bevy system.
    pub world: PhysicsWorld,
}
