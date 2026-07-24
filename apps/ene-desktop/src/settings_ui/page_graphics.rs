//! Graphics settings page.
//!
//! Quality preset and language selection for desktop rendering.
use super::widgets::{SettingsAction, apply_action, format_quality_label};
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
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "language"));
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
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "graphics-quality"
            ));
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::GraphicsQualityDown,
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
                egui::Label::new(format_quality_label(
                    settings.language,
                    settings.graphics.quality,
                )),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::GraphicsQualityUp,
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
