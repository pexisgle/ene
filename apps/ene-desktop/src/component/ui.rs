//! ECS Components for the settings UI.
//!
//! Each component owns a small slice of the runtime UI state. The
//! [`SettingsUiBundle`](super::character::SettingsUiBundle) (see
//! `plugin/ui_plugin.rs`) assembles them on a single entity so the
//! runtime can call into the egui render functions via
//! `world.get::<&UiPage>(entity)` instead of carrying a
//! side-table of fields on `SettingsUi`.
//!
//! Migration goal: the legacy
//! `apps/ene-desktop/src/settings_ui/mod.rs::SettingsUi` struct is
//! split into the components below so that
//! `apply_action` can run as a bevy system consuming a
//! [`SettingsActionEvent`](crate::event::ui_action::SettingsActionEvent)
//! message instead of being a free function with seven
//! parameters.
use std::time::Instant;

use bevy_ecs::prelude::*;

use crate::character_state::{AnimationControl, EmotionQueue};
use crate::settings::UiState;
use crate::settings_ui::{PageKind, input::SettingsInputState};

/// Marker placed on the single settings-window entity. Phase 5 keeps
/// exactly one UI entity for the whole process; Phase 6 will spawn
/// extra entities for the platform / AI windows.
#[derive(Component, Default)]
pub struct UiWindow;

/// Currently-visible page. Mirrors the legacy
/// `SettingsUi::current_page` field.
#[derive(Component, Default)]
#[expect(dead_code, reason = "Mirror of legacy SettingsUi.current_page")]
pub struct UiPage(pub PageKind);

/// Editable text-field buffers. Mirrors the legacy
/// `SettingsUi::input` field. Mirrored on the on-disk
/// `CharacterSettings` via `sync_from_settings` when the window
/// transitions hidden → visible.
#[derive(Component, Default)]
#[expect(dead_code, reason = "Mirror of legacy SettingsUi.input")]
pub struct UiInputDrafts(pub SettingsInputState);

/// Animation play / pause toggle. Mirrors the legacy
/// `SettingsUi::animation` field.
#[derive(Component, Default)]
#[expect(dead_code, reason = "Mirror of legacy SettingsUi.animation")]
pub struct UiAnimation(pub AnimationControl);

/// Pending emotion commands emitted by the AI bridge or the
/// settings UI's manual-expression buttons. Mirrors the legacy
/// `SettingsUi::emotion_queue` field; the
/// `apply_emotions_system` (in `AiPlugin`) drains this queue
/// into the legacy per-frame buffer.
#[derive(Component, Default)]
pub struct UiEmotionQueue(pub EmotionQueue);

/// Runtime-startup `Instant` so that `now_secs` used by
/// `apply_action` is consistent across systems. Mirrors the legacy
/// `SettingsUi::started_at` field.
#[derive(Component)]
#[expect(dead_code, reason = "Mirror of legacy SettingsUi.started_at")]
pub struct UiStartedAt(pub Instant);

impl Default for UiStartedAt {
    fn default() -> Self {
        Self(Instant::now())
    }
}

/// Persistent UI state (visibility, debug toggles, AI chat
/// scratch). Mirrors `SettingsUi::UiState` field but lifted from
/// lifted from the legacy `AppState` field into the bevy world. The runtime reads
/// / writes this via `Mut<UiStateComponent>`.
#[derive(Component, Default)]
pub struct UiStateComponent(pub UiState);

/// Bundle that assembles every UI component on a single entity.
/// `SettingsUiPlugin::build` adds a startup system that spawns one
/// of these into the bevy world.
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
        assert!(matches!(bundle.page.0, PageKind::Character));
        assert_eq!(bundle.input.0.ai_embedding_provider, "");
        assert!(!bundle.state.0.settings_window_visible);
        assert!(bundle.emotion_queue.0.commands.is_empty());
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
