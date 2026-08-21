//! [`SettingsAction`](crate::settings_ui::widgets::SettingsAction) is
//! the single funnel through which buttons, hotkeys, and direct egui
//! field changes mutate `CharacterSettings`. This message lifts it
//! into the bevy ECS so `apply_settings_action_system` can consume it
//! in the `Settings` schedule set.
use bevy_ecs::prelude::*;

/// One settings action queued by the UI layer. The
/// `apply_settings_action_system` reads these messages and mutates
/// the [`SettingsBundle`](crate::component::ui::SettingsUiBundle)'s
/// components in place.
///
/// `Clone` but not `Copy` because the character-card editor variants
/// of [`SettingsAction`](crate::settings_ui::widgets::SettingsAction)
/// carry an owned path `String`.
#[derive(Message, Debug, Clone)]
pub struct SettingsActionEvent {
    #[expect(dead_code, reason = "Read by Phase 6 per-action consumer systems")]
    pub action: crate::settings_ui::widgets::SettingsAction,
}
