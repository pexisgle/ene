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
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::language());
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::LanguageDown,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(match settings.language {
                    crate::settings::Language::En => "English",
                    crate::settings::Language::Ja => "日本語",
                }),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::LanguageUp,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::target_fps());
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::TargetFpsDown,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_fps_label(
                    settings.language,
                    settings.graphics.target_fps,
                )),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::TargetFpsUp,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::shadow_quality());
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::ShadowQualityDown,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_shadow_label(
                    settings.language,
                    settings.graphics.shadow_quality,
                )),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::ShadowQualityUp,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::antialiasing());
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::AntialiasingModeDown,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_aa_label(
                    settings.language,
                    settings.graphics.antialiasing_mode,
                )),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::AntialiasingModeUp,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
        });
    });
}
