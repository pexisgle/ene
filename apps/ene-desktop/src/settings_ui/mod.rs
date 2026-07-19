//! Settings UI — the tabbed window that mirrors the legacy
//! Bevy `apps/ene-desktop/src/settings_ui/`.
//!
//! The runtime owns a single [`SettingsUi`] per `UiWindow`. Each
//! frame the `UiWindow` calls [`SettingsUi::render`] with the live
//! `&mut CharacterSettings` and the `Arc<AiBridge>`.
pub mod input;
pub mod page_ai;
pub mod page_character;
pub mod page_debug;
pub mod page_features;
pub mod page_graphics;
pub mod page_memory;
pub mod widgets;

pub use input::SettingsInputState;

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionQueue};
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind {
    #[default]
    Character,
    Graphics,
    Ai,
    Features,
    Memory,
    Debug,
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
        world: &mut World,
        ui_entity: Entity,
        now_secs: f64,
    ) {
        apply_egui_visuals(ui.ctx());

        // Top-level page tab strip.
        ui.horizontal(|ui| {
            for page in [
                PageKind::Character,
                PageKind::Graphics,
                PageKind::Ai,
                PageKind::Features,
                PageKind::Memory,
                PageKind::Debug,
            ] {
                let label = match page {
                    PageKind::Character => crate::i18n::character(),
                    PageKind::Graphics => crate::i18n::graphics(),
                    PageKind::Debug => crate::i18n::debug(),
                    PageKind::Ai => crate::i18n::ai(),
                    PageKind::Features => crate::i18n::features(),
                    PageKind::Memory => i18n_embed_fl::fl!(crate::i18n::loader(), "memory"),
                };
                if ui
                    .selectable_label(self.current_page == page, label)
                    .clicked()
                {
                    self.current_page = page;
                }
            }
        });
        ui.separator();
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
                page_graphics::render(ui, settings, &mut self.animation, ai, world, ui_entity);
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
            PageKind::Features => page_features::render(ui, settings, ai),
            PageKind::Memory => page_memory::render(ui, ai, world, ui_entity),
            PageKind::Debug => {
                page_debug::render(ui, settings, &mut self.animation, ai, world, ui_entity);
            }
        }
    }
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self::new()
    }
}

fn noto_sans_jp_font_definitions() -> Option<&'static egui::FontDefinitions> {
    static FONTS: OnceLock<Option<egui::FontDefinitions>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let assets_dir = ene_config::paths::assets_dir();
            let font_path = assets_dir.join("fonts").join("NotoSansJP-Regular.ttf");
            if !font_path.exists() {
                tracing::warn!("Font file does not exist at {:?}", font_path);
                return None;
            }
            let font_data = match std::fs::read(&font_path) {
                Ok(data) => data,
                Err(error) => {
                    tracing::warn!(%error, path = ?font_path, "Failed to read font file");
                    return None;
                }
            };

            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "NotoSansJP".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                prop.insert(0, "NotoSansJP".to_owned());
            }
            if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                mono.insert(0, "NotoSansJP".to_owned());
            }
            tracing::info!("Successfully loaded NotoSansJP-Regular.ttf for egui");
            Some(fonts)
        })
        .as_ref()
}

/// Install `NotoSansJP` into a dedicated egui context. Each winit window
/// owns its own [`egui::Context`], so fonts must be applied per context
/// (not once process-wide).
pub fn apply_egui_fonts(ctx: &egui::Context) {
    if let Some(fonts) = noto_sans_jp_font_definitions() {
        ctx.set_fonts(fonts.clone());
    }
}

/// Apply the legacy dark theme tokens. The v1 Bevy build uses
/// these exact RGB values; v2 keeps the visual identity stable so
/// screenshots and docs that reference the colors stay valid.
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
