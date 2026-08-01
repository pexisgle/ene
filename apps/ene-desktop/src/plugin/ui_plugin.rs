//! # Settings UI plugin
//!
//! This plugin is responsible for:
//!
//! 1. Adding the [`SettingsActionEvent`](crate::event::ui_action::SettingsActionEvent)
//!    message queue to the [`App`].
//! 2. Spawning the single
//!    [`SettingsUiBundle`](crate::component::ui::SettingsUiBundle)
//!    entity during `Startup` so the page render functions can
//!    read / write components through `World::get` and
//!    `World::get_mut`.
//! 3. Providing the `apply_settings_action_system` drain so the
//!    message queue does not accumulate unread events.
use bevy_app::{App, Plugin, Startup, Update};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;

use crate::component::ui::{SettingsUiBundle, UiStartedAt};
use crate::event::ui_action::SettingsActionEvent;
use crate::schedule::AppSet;
use crate::system::ui_dispatcher::apply_settings_action_system;
use std::time::Instant;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SettingsActionEvent>();
        app.add_systems(Startup, spawn_settings_ui_window);
        app.add_systems(
            Update,
            apply_settings_action_system.in_set(AppSet::Settings),
        );
    }
}

/// Spawn the single settings-window entity. The entity is queried
/// by `SettingsUi::render` (via `runtime.rs`) and by the
/// `apply_settings_action_system` that consumes
/// `SettingsActionEvent` messages.
fn spawn_settings_ui_window(mut commands: Commands) {
    commands.spawn(SettingsUiBundle {
        started_at: UiStartedAt(Instant::now()),
        ..SettingsUiBundle::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_can_be_added() {
        let mut app = App::new();
        app.add_plugins(UiPlugin);
        // Trigger the Startup schedule so `spawn_settings_ui_window`
        // runs; the bevy App otherwise defers `Startup` until
        // the first `update`.
        app.world_mut().run_schedule(bevy_app::Startup);
        let count = app
            .world_mut()
            .query_filtered::<Entity, With<crate::component::ui::UiWindow>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 1);
    }
}
