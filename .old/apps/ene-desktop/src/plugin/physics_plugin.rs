//! Bridges [`crate::physics::PhysicsWorld`] into the bevy ECS.

use bevy_app::prelude::{App, Plugin, Startup, Update};
use bevy_ecs::schedule::IntoScheduleConfigs;

use crate::resource::physics::PhysicsWorldResource;
use crate::schedule::AppSet;
use crate::system::physics::{attach_bone_colliders_system, step_physics_system};

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsWorldResource>();
        app.add_systems(Startup, attach_bone_colliders_system);
        app.add_systems(Update, step_physics_system.in_set(AppSet::Animation));
    }
}
