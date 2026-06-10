use super::{
    SettingsButtonAction, SettingsInputState, SettingsValueKind,
    widgets::{apply_action, render_cycle_row, render_numeric_row, render_toggle_row},
};
use crate::ai_bridge::EneRequestEvent;
use crate::app_config::CharacterSettings;
use crate::character::{CharacterAnimationControl, EmotionCommand, EmotionQueue};
use bevy::prelude::*;
use bevy_egui::egui;

pub fn render_character_page(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation_control: &mut CharacterAnimationControl,
    ai_request_writer: &mut MessageWriter<EneRequestEvent>,
    input_state: &mut SettingsInputState,
    emotion_queue: &mut EmotionQueue,
    time: &Time,
) {
    ui.vertical(|ui| {
        if let Some(action) = render_cycle_row(
            ui,
            "Character",
            SettingsValueKind::Character,
            settings,
            animation_control,
            SettingsButtonAction::PrevCharacter,
            SettingsButtonAction::NextCharacter,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
        }

        if let Some(action) = render_cycle_row(
            ui,
            "Motion",
            SettingsValueKind::Motion,
            settings,
            animation_control,
            SettingsButtonAction::PrevMotion,
            SettingsButtonAction::NextMotion,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
        }

        if let Some(action) = render_toggle_row(
            ui,
            "Animation",
            SettingsValueKind::AnimationState,
            settings,
            animation_control,
            SettingsButtonAction::TogglePlay,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
        }

        render_linux_only_settings(ui, settings, animation_control, ai_request_writer);

        if let Some(action) = render_numeric_row(
            ui,
            "LookAt Strength",
            &mut input_state.look_at_strength,
            SettingsValueKind::LookAtStrength,
            settings,
            SettingsButtonAction::LookAtStrengthDown,
            SettingsButtonAction::LookAtStrengthUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
            input_state.look_at_strength =
                format!("{:.2}", settings.character_state.look_at_strength);
        }

        if let Some(action) = render_numeric_row(
            ui,
            "Model Scale",
            &mut input_state.model_scale,
            SettingsValueKind::ModelScale,
            settings,
            SettingsButtonAction::ModelScaleDown,
            SettingsButtonAction::ModelScaleUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
            input_state.model_scale = format!("{:.2}", settings.character_state.model_scale);
        }

        if let Some(action) = render_numeric_row(
            ui,
            "Character Pos X",
            &mut input_state.character_pos_x,
            SettingsValueKind::CharacterPositionX,
            settings,
            SettingsButtonAction::CharacterPosXDown,
            SettingsButtonAction::CharacterPosXUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
            input_state.character_pos_x =
                format!("{:+.2}", settings.character_state.character_position.x);
        }

        if let Some(action) = render_numeric_row(
            ui,
            "Character Pos Y",
            &mut input_state.character_pos_y,
            SettingsValueKind::CharacterPositionY,
            settings,
            SettingsButtonAction::CharacterPosYDown,
            SettingsButtonAction::CharacterPosYUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
            input_state.character_pos_y =
                format!("{:+.2}", settings.character_state.character_position.y);
        }

        if let Some(action) = render_numeric_row(
            ui,
            "Character Pos Z",
            &mut input_state.character_pos_z,
            SettingsValueKind::CharacterPositionZ,
            settings,
            SettingsButtonAction::CharacterPosZDown,
            SettingsButtonAction::CharacterPosZUp,
        ) {
            apply_action(action, settings, animation_control, ai_request_writer);
            input_state.character_pos_z =
                format!("{:+.2}", settings.character_state.character_position.z);
        }

        ui.separator();
        ui.label("Manual Expressions (Test)");
        ui.horizontal(|ui| {
            for emotion in ["happy", "sad", "angry", "relaxed", "surprised", "neutral"] {
                if ui.button(emotion).clicked() {
                    emotion_queue.commands.push_back(EmotionCommand {
                        emotion: emotion.to_string(),
                        target_time: time.elapsed_secs_f64(),
                        hold_secs: 4.0,
                    });
                }
            }
        });
    });
}

#[cfg(target_os = "linux")]
fn render_linux_only_settings(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation_control: &mut CharacterAnimationControl,
    ai_request_writer: &mut MessageWriter<EneRequestEvent>,
) {
    if let Some(action) = render_toggle_row(
        ui,
        "Debug Overlay",
        SettingsValueKind::DebugOverlay,
        settings,
        animation_control,
        SettingsButtonAction::ToggleDebugOverlay,
    ) {
        apply_action(action, settings, animation_control, ai_request_writer);
    }

    if let Some(action) = render_cycle_row(
        ui,
        "Mask Downsample",
        SettingsValueKind::MaskRenderDownsample,
        settings,
        animation_control,
        SettingsButtonAction::MaskDownsampleDown,
        SettingsButtonAction::MaskDownsampleUp,
    ) {
        apply_action(action, settings, animation_control, ai_request_writer);
    }
}

#[cfg(not(target_os = "linux"))]
fn render_linux_only_settings(
    _ui: &mut egui::Ui,
    _settings: &mut CharacterSettings,
    _animation_control: &mut CharacterAnimationControl,
    _ai_request_writer: &mut MessageWriter<EneRequestEvent>,
) {
}
