//! Settings action message.
//!
//! The legacy `SettingsAction` enum (in
//! `apps/ene-desktop/src/settings_ui/widgets.rs`) was the single
//! funnel through which buttons, hotkeys, and direct egui field
//! changes mutated `CharacterSettings`. Phase 5 lifts it into the
//! bevy ECS as a [`Message`] so that
//! `apply_settings_action_system` can consume it in the
//! `Settings` schedule set.
use bevy_ecs::prelude::*;

/// One settings action queued by the UI layer. The
/// `apply_settings_action_system` reads these messages and mutates
/// the [`SettingsBundle`](crate::component::ui::SettingsUiBundle)'s
/// components in place.
#[derive(Message, Debug, Clone, Copy)]
#[expect(dead_code, reason = "Consumed by Phase 5 consumer system")]
pub struct SettingsActionEvent {
    pub action: crate::settings_ui::widgets::SettingsAction,
}
