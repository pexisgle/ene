//! Chat window plugin.

use bevy_app::{App, Plugin, Startup};
use bevy_ecs::prelude::*;

use crate::component::chat::ChatUiBundle;

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_chat_window);
    }
}

fn spawn_chat_window(mut commands: Commands) {
    commands.spawn(ChatUiBundle::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::chat::ChatWindow;
    use bevy_app::App;

    #[test]
    fn plugin_spawns_chat_entity() {
        let mut app = App::new();
        app.add_plugins(ChatPlugin);
        app.world_mut().run_schedule(bevy_app::Startup);
        let count = app
            .world_mut()
            .query_filtered::<Entity, With<ChatWindow>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 1);
    }
}
