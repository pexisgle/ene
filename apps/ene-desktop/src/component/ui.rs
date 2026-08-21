//! ECS Components for the settings UI.
//!
//! Each component owns a small slice of the runtime UI state. The
//! [`SettingsUiBundle`](super::character::SettingsUiBundle) (see
//! `plugin/ui_plugin.rs`) assembles them on a single entity so the
//! runtime can call into the egui render functions via
//! `world.get::<&UiPage>(entity)` instead of carrying a
//! side-table of fields on `SettingsUi`.
//!
//! The state below is owned by these components so `apply_action`
//! (a bevy system consuming a
//! [`SettingsActionEvent`](crate::event::ui_action::SettingsActionEvent)
//! message) can read it from the entity instead of from `SettingsUi`.
use std::time::Instant;

use bevy_ecs::prelude::*;

use crate::character_state::{AnimationControl, EmotionQueue};
use crate::settings::UiState;
use crate::settings_ui::{PageKind, input::SettingsInputState};

/// Marker placed on the single settings-window entity.
#[derive(Component, Default)]
pub struct UiWindow;

/// Mirrors `SettingsUi::current_page`.
#[derive(Component, Default)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Mirror of legacy SettingsUi.current_page")
)]
pub struct UiPage(pub PageKind);

/// Mirrors `SettingsUi::input`; synced from the on-disk `CharacterSettings`
/// via `sync_from_settings` when the window transitions hidden → visible.
#[derive(Component, Default)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Mirror of legacy SettingsUi.input")
)]
pub struct UiInputDrafts(pub SettingsInputState);

/// Mirrors `SettingsUi::animation`.
#[derive(Component, Default)]
pub struct UiAnimation(pub AnimationControl);

/// Mirrors `SettingsUi::emotion_queue`; `apply_emotions_system` (in
/// `AiPlugin`) drains this queue into `EmotionPipelineState::pending`.
#[derive(Component, Default)]
pub struct UiEmotionQueue(pub EmotionQueue);

/// Keeps `now_secs` used by `apply_action` consistent across systems;
/// mirrors `SettingsUi::started_at`.
#[derive(Component)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Mirror of legacy SettingsUi.started_at")
)]
pub struct UiStartedAt(pub Instant);

impl Default for UiStartedAt {
    fn default() -> Self {
        Self(Instant::now())
    }
}

/// Persistent UI state (visibility, debug toggles, AI chat
/// scratch); mirrors `SettingsUi::UiState`. The runtime reads
/// / writes this via `Mut<UiStateComponent>`.
///
/// Runtime health (`runtime_startup_error`, `runtime_disconnected`,
/// `reconnect_attempted`) lives here exclusively: the actor failure is
/// handed over via [`crate::resource::startup::RuntimeStartupError`] and
/// reconnect results are written directly into this component.
#[derive(Component, Default)]
pub struct UiStateComponent(pub UiState);

/// `SettingsUiPlugin::build` adds a startup system that spawns one of these
/// into the bevy world.
#[derive(Bundle, Default)]
pub struct SettingsUiBundle {
    pub window: UiWindow,
    pub page: UiPage,
    pub input: UiInputDrafts,
    pub animation: UiAnimation,
    pub emotion_queue: UiEmotionQueue,
    pub started_at: UiStartedAt,
    pub state: UiStateComponent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_defaults_are_safe() {
        let bundle = SettingsUiBundle::default();
        assert!(
            matches!(bundle.page.0, PageKind::Overview),
            "the settings window opens on the Overview page"
        );
        assert_eq!(bundle.input.0.ai_embedding_provider, "");
        assert!(!bundle.state.0.settings_window_visible);
        assert!(bundle.emotion_queue.0.commands.is_empty());
        assert!(bundle.animation.0.playing);
    }

    #[test]
    fn ui_started_at_defaults_to_now() {
        let before = Instant::now();
        let s = UiStartedAt::default();
        let after = Instant::now();
        assert!(s.0 >= before);
        assert!(s.0 <= after);
    }
}
