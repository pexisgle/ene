use super::{
    SettingsButtonAction, SettingsValueKind,
    widgets::{apply_action, render_cycle_row},
};
use crate::ai_bridge::EneRequestEvent;
use crate::app_config::CharacterSettings;
use crate::character::CharacterAnimationControl;
use bevy::prelude::*;
use bevy_egui::egui;

pub fn render_graphics_page(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation_control: &mut CharacterAnimationControl,
    ai_request_writer: &mut MessageWriter<EneRequestEvent>,
) {
    ui.vertical(|ui| {
        if let Some(action) = render_cycle_row(
            ui,
            "Target FPS",
            SettingsValueKind::TargetFps,
            settings,
            animation_control,
            SettingsButtonAction::TargetFpsDown,
            SettingsButtonAction::TargetFpsUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
        }

        if let Some(action) = render_cycle_row(
            ui,
            "Shadow Quality",
            SettingsValueKind::ShadowQuality,
            settings,
            animation_control,
            SettingsButtonAction::ShadowQualityDown,
            SettingsButtonAction::ShadowQualityUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
        }

        if let Some(action) = render_cycle_row(
            ui,
            "Antialiasing",
            SettingsValueKind::AntialiasingMode,
            settings,
            animation_control,
            SettingsButtonAction::AntialiasingModeDown,
            SettingsButtonAction::AntialiasingModeUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
        }
    });
}
