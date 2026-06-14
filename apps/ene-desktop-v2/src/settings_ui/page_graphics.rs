//! Graphics settings page.
//!
//! Three cycle rows (target FPS, shadow quality, antialiasing) that
//! drive the same [`crate::settings::GraphicsSettings`] fields the
//! legacy Bevy `page_graphics.rs` exposed.
use super::widgets::{
    SettingsAction, apply_action, format_aa_label, format_fps_label, format_shadow_label,
};
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label("Target FPS");
            if ui.button("<").clicked() {
                apply_action(SettingsAction::TargetFpsDown, settings, animation, ai);
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_fps_label(settings.graphics.target_fps)),
            );
            if ui.button(">").clicked() {
                apply_action(SettingsAction::TargetFpsUp, settings, animation, ai);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Shadow Quality");
            if ui.button("<").clicked() {
                apply_action(SettingsAction::ShadowQualityDown, settings, animation, ai);
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_shadow_label(settings.graphics.shadow_quality)),
            );
            if ui.button(">").clicked() {
                apply_action(SettingsAction::ShadowQualityUp, settings, animation, ai);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Antialiasing");
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::AntialiasingModeDown,
                    settings,
                    animation,
                    ai,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_aa_label(settings.graphics.antialiasing_mode)),
            );
            if ui.button(">").clicked() {
                apply_action(SettingsAction::AntialiasingModeUp, settings, animation, ai);
            }
        });
    });
}
