//! Physics plugin: bridges [`crate::physics::PhysicsWorld`] into
//! the bevy ECS.
//!
//! Registers the [`PhysicsWorldResource`], attaches the per-entity
//! rapier handle components in `Startup`, and steps the physics
//! world once per frame.

use bevy_app::prelude::{App, Plugin, Startup, Update};
use bevy_ecs::schedule::IntoScheduleConfigs;

use crate::resource::physics::PhysicsWorldResource;
use crate::schedule::AppSet;
use crate::system::physics::{attach_bone_colliders_system, step_physics_system};

/// Plugin that populates the per-entity rapier handle components
/// and steps the physics world once per frame.
pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsWorldResource>();
        app.add_systems(Startup, attach_bone_colliders_system);
        app.add_systems(Update, step_physics_system.in_set(AppSet::Animation));
    }
}
