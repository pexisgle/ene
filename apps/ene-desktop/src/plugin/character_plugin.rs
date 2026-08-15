//! Spawns a single [`CharacterRoot`] entity with every data-only
//! component at its `Default` value. The actual VRM rendering stays
//! on [`crate::character::CharacterRenderer`] via `AppState::character`.
use bevy_app::{App, Plugin, Startup};
use bevy_ecs::prelude::*;

use crate::component::character::{
    BoneColliders, CharacterCamera, CharacterRoot, CharacterTransform, EmotionChannel, LookAt,
    MotionState, SpringBoneState, VrmModelHandle,
};
use crate::component::transform::{GlobalTransform, Transform};

/// Bundle of components attached to the primary character entity.
#[derive(Bundle, Default)]
pub struct CharacterBundle {
    pub root: CharacterRoot,
    pub model: VrmModelHandle,
    pub motion: MotionState,
    pub spring_bone: SpringBoneState,
    pub camera: CharacterCamera,
    pub look_at: LookAt,
    pub emotion: EmotionChannel,
    pub colliders: BoneColliders,
    pub transform: CharacterTransform,
    pub scene_transform: Transform,
    pub scene_global: GlobalTransform,
}

#[derive(Default)]
pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        // Spawn the character entity on first startup. The actual
        // VRM data is loaded later by `Runtime::resumed` once a GPU
        // device exists.
        app.add_systems(Startup, spawn_character_entity);
    }
}

/// Idempotent — running it twice is a no-op.
fn spawn_character_entity(mut commands: Commands) {
    commands.spawn(CharacterBundle::default());
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;

    #[test]
    fn plugin_can_be_added() {
        let mut app = App::new();
        app.add_plugins(CharacterPlugin);
        // The Startup system runs in the default schedule runner
        // that `App::new` provides.
        app.world_mut().run_schedule(bevy_app::Startup);
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<CharacterRoot>>();
        assert_eq!(q.iter(app.world()).count(), 1);
    }

    #[test]
    fn bundle_defaults_are_safe() {
        let bundle = CharacterBundle::default();
        assert!(bundle.model.model.is_none());
        assert!(bundle.motion.asset.is_none());
        assert!(bundle.spring_bone.0.is_none());
        assert!(bundle.colliders.0.is_empty());
        assert_eq!(bundle.transform.scale, 0.0);
    }
}
