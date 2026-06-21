//! Settings UI — the 3-page tabbed window that mirrors the legacy
//! Bevy `apps/ene-desktop/src/settings_ui/`.
//!
//! The runtime owns a single [`SettingsUi`] per `UiWindow`. Each
//! frame the `UiWindow` calls [`SettingsUi::render`] with the live
//! `&mut CharacterSettings` and the `Arc<AiBridge>`.
pub mod input;
pub mod page_ai;
pub mod page_character;
pub mod page_debug;
pub mod page_graphics;
pub mod widgets;

pub use input::SettingsInputState;

use std::sync::Arc;
use std::time::Instant;

use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionQueue};
use crate::settings::CharacterSettings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind {
    #[default]
    Character,
    Graphics,
    Ai,
    Debug,
}

impl PageKind {
    pub fn label(self) -> &'static str {
        match self {
            PageKind::Character => "Character",
            PageKind::Graphics => "Graphics",
            PageKind::Ai => "AI",
            PageKind::Debug => "Debug",
        }
    }
}

#[derive(Debug)]
pub struct SettingsUi {
    pub current_page: PageKind,
    pub input: SettingsInputState,
    pub animation: AnimationControl,
    pub emotion_queue: EmotionQueue,
    /// When the runtime was constructed. Used for `now_secs` in
    /// emotion-queue timestamps.
    pub started_at: Instant,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self {
            current_page: PageKind::Character,
            input: SettingsInputState::new(),
            animation: AnimationControl::new(),
            emotion_queue: EmotionQueue::default(),
            started_at: Instant::now(),
        }
    }

    /// Switch the visible page. Used by the runtime to jump to the
    /// AI page when a `PermissionRequired` or `UserInputRequired`
    /// event arrives (A.2 follow-up: settings tray menu opens to a
    /// specific page).
    pub fn show(&mut self, page: PageKind) {
        self.current_page = page;
    }

    /// Mirror the on-disk `CharacterSettings` into the editable text
    /// buffers. The runtime calls this when the settings window
    /// transitions from hidden → visible.
    pub fn sync_from_settings(
        &mut self,
        settings: &CharacterSettings,
        ui_state: &crate::settings::UiState,
    ) {
        self.input.sync_from_settings(settings, ui_state);
    }

    /// Render the full settings window. The caller is expected to
    /// have already opened an egui pass and supplied a `Ui` (via
    /// `egui::CentralPanel::show_inside`).
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut CharacterSettings,
        ai: &Arc<AiBridge>,
        world: &mut hecs::World,
        ui_entity: hecs::Entity,
    ) {
        apply_egui_visuals(ui.ctx());

        // Top-level page tab strip.
        ui.horizontal(|ui| {
            for page in [
                PageKind::Character,
                PageKind::Graphics,
                PageKind::Ai,
                PageKind::Debug,
            ] {
                let label = page.label();
                if ui
                    .selectable_label(self.current_page == page, label)
                    .clicked()
                {
                    self.current_page = page;
                }
            }
        });
        ui.separator();

        let now_secs = self.started_at.elapsed().as_secs_f64();
        match self.current_page {
            PageKind::Character => page_character::render(
                ui,
                settings,
                &mut self.animation,
                ai,
                &mut self.input,
                &mut self.emotion_queue,
                now_secs,
                world,
                ui_entity,
            ),
            PageKind::Graphics => {
                page_graphics::render(ui, settings, &mut self.animation, ai, world, ui_entity)
            }
            PageKind::Ai => page_ai::render(
                ui,
                settings,
                &mut self.animation,
                ai,
                &mut self.input,
                world,
                ui_entity,
            ),
            PageKind::Debug => {
                page_debug::render(ui, settings, &mut self.animation, ai, world, ui_entity)
            }
        }
    }
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply the legacy dark theme tokens. The legacy Bevy code uses
/// exactly these RGB values; v2 keeps the visual identity stable so
/// screenshots / docs that reference the colors remain valid.
pub fn apply_egui_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(26, 28, 33);
    visuals.window_fill = egui::Color32::from_rgb(20, 22, 28);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(30, 33, 38);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(38, 42, 50);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(52, 57, 66);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(72, 77, 89);
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(220, 224, 232);
    visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(240, 243, 248);
    visuals.widgets.active.fg_stroke.color = egui::Color32::from_rgb(247, 248, 250);
    ctx.set_visuals(visuals);
}
