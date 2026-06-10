use super::{SettingsButtonAction, SettingsValueKind};
use crate::app_config::CharacterSettings;
use crate::character::CharacterAnimationControl;
use bevy::prelude::MessageWriter;
use bevy_egui::egui;

pub fn apply_action(
    action: SettingsButtonAction,
    settings: &mut CharacterSettings,
    animation_control: &mut CharacterAnimationControl,
    ai_request_writer: &mut MessageWriter<crate::ai_bridge::EneRequestEvent>,
) {
    match action {
        SettingsButtonAction::PrevCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                -1,
            );
            settings.select_character(idx);
        }
        SettingsButtonAction::NextCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                1,
            );
            settings.select_character(idx);
        }
        SettingsButtonAction::PrevMotion => {
            settings.character_state.selected_motion = cycle_index(
                settings.character_state.selected_motion,
                settings.current_entry().motion_names.len(),
                -1,
            );
            settings.character_state.motion_override = None;
            settings.character_state.needs_respawn = true;
            settings.mark_dirty();
        }
        SettingsButtonAction::NextMotion => {
            settings.character_state.selected_motion = cycle_index(
                settings.character_state.selected_motion,
                settings.current_entry().motion_names.len(),
                1,
            );
            settings.character_state.motion_override = None;
            settings.character_state.needs_respawn = true;
            settings.mark_dirty();
        }
        SettingsButtonAction::TogglePlay => {
            animation_control.toggle_playing();
        }
        #[cfg(target_os = "linux")]
        SettingsButtonAction::ToggleDebugOverlay => {
            settings.ui.debug_overlay_visible = !settings.ui.debug_overlay_visible;
        }
        #[cfg(target_os = "linux")]
        SettingsButtonAction::MaskDownsampleDown => {
            settings.graphics.mask_render_downsample =
                cycle_mask_render_downsample(settings.graphics.mask_render_downsample, -1);
        }
        #[cfg(target_os = "linux")]
        SettingsButtonAction::MaskDownsampleUp => {
            settings.graphics.mask_render_downsample =
                cycle_mask_render_downsample(settings.graphics.mask_render_downsample, 1);
        }
        SettingsButtonAction::TargetFpsDown => {
            settings.graphics.target_fps = cycle_target_fps(settings.graphics.target_fps, -1);
        }
        SettingsButtonAction::TargetFpsUp => {
            settings.graphics.target_fps = cycle_target_fps(settings.graphics.target_fps, 1);
        }
        SettingsButtonAction::ShadowQualityDown => {
            settings.graphics.shadow_quality =
                cycle_shadow_quality(settings.graphics.shadow_quality, -1);
        }
        SettingsButtonAction::ShadowQualityUp => {
            settings.graphics.shadow_quality =
                cycle_shadow_quality(settings.graphics.shadow_quality, 1);
        }
        SettingsButtonAction::AntialiasingModeDown => {
            settings.graphics.antialiasing_mode =
                cycle_antialiasing_mode(settings.graphics.antialiasing_mode, -1);
        }
        SettingsButtonAction::AntialiasingModeUp => {
            settings.graphics.antialiasing_mode =
                cycle_antialiasing_mode(settings.graphics.antialiasing_mode, 1);
        }
        SettingsButtonAction::LookAtStrengthDown => {
            adjust_f32(&mut settings.character_state.look_at_strength, -0.05);
        }
        SettingsButtonAction::LookAtStrengthUp => {
            adjust_f32(&mut settings.character_state.look_at_strength, 0.05);
        }
        SettingsButtonAction::ModelScaleDown => {
            adjust_f32(&mut settings.character_state.model_scale, -0.05);
        }
        SettingsButtonAction::ModelScaleUp => {
            adjust_f32(&mut settings.character_state.model_scale, 0.05);
        }
        SettingsButtonAction::CharacterPosXDown => {
            adjust_f32(&mut settings.character_state.character_position.x, -0.05);
        }
        SettingsButtonAction::CharacterPosXUp => {
            adjust_f32(&mut settings.character_state.character_position.x, 0.05);
        }
        SettingsButtonAction::CharacterPosYDown => {
            adjust_f32(&mut settings.character_state.character_position.y, -0.05);
        }
        SettingsButtonAction::CharacterPosYUp => {
            adjust_f32(&mut settings.character_state.character_position.y, 0.05);
        }
        SettingsButtonAction::CharacterPosZDown => {
            adjust_f32(&mut settings.character_state.character_position.z, -0.05);
        }
        SettingsButtonAction::CharacterPosZUp => {
            adjust_f32(&mut settings.character_state.character_position.z, 0.05);
        }
        SettingsButtonAction::SendAiChat => {
            send_ai_request(settings, ai_request_writer);
        }
    }

    settings.clamp_runtime_values();
    settings.mark_dirty();
}

pub fn render_cycle_row(
    ui: &mut egui::Ui,
    label: &str,
    value_kind: SettingsValueKind,
    settings: &CharacterSettings,
    animation_control: &CharacterAnimationControl,
    down_action: SettingsButtonAction,
    up_action: SettingsButtonAction,
) -> Option<SettingsButtonAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.button("<").clicked() {
            action = Some(down_action);
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(value_kind.current_text(settings, animation_control)),
        );
        if ui.button(">").clicked() {
            action = Some(up_action);
        }
    });
    action
}

pub fn render_toggle_row(
    ui: &mut egui::Ui,
    label: &str,
    value_kind: SettingsValueKind,
    settings: &CharacterSettings,
    animation_control: &CharacterAnimationControl,
    toggle_action: SettingsButtonAction,
) -> Option<SettingsButtonAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.button("Toggle").clicked() {
            action = Some(toggle_action);
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(value_kind.current_text(settings, animation_control)),
        );
    });
    action
}

pub fn render_numeric_row(
    ui: &mut egui::Ui,
    label: &str,
    text_buffer: &mut String,
    value_kind: SettingsValueKind,
    settings: &mut CharacterSettings,
    down_action: SettingsButtonAction,
    up_action: SettingsButtonAction,
) -> Option<SettingsButtonAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.button("-").clicked() {
            action = Some(down_action);
        }
        let response = ui.add(egui::TextEdit::singleline(text_buffer).desired_width(220.0));
        let commit = response.changed()
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if commit && value_kind.apply_input(text_buffer.trim(), settings).is_ok() {
            settings.clamp_runtime_values();
            settings.mark_dirty();
        }
        if ui.button("+").clicked() {
            action = Some(up_action);
        }
    });
    action
}

fn cycle_index(index: usize, len: usize, step: isize) -> usize {
    ((index as isize + step).rem_euclid(len as isize)) as usize
}

fn adjust_f32(value: &mut f32, delta: f32) {
    *value += delta;
}

fn send_ai_request(
    settings: &mut CharacterSettings,
    ai_request_writer: &mut MessageWriter<crate::ai_bridge::EneRequestEvent>,
) {
    let user_input = settings.ui.ai_chat_input.trim();
    if user_input.is_empty() {
        return;
    }

    ai_request_writer.write(crate::ai_bridge::EneRequestEvent {
        user_input: user_input.to_string(),
    });
    settings.ui.ai_chat_input.clear();
    settings.ui.ai_latest_response.clear();
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cycle_mask_render_downsample(current: u32, step: isize) -> u32 {
    crate::app_config::cycle_mask_render_downsample(current, step)
}

fn cycle_target_fps(current: u32, step: isize) -> u32 {
    crate::app_config::cycle_target_fps(current, step)
}

fn cycle_shadow_quality(
    current: crate::app_config::ShadowQuality,
    step: isize,
) -> crate::app_config::ShadowQuality {
    crate::app_config::cycle_shadow_quality(current, step)
}

fn cycle_antialiasing_mode(
    current: crate::app_config::AntialiasingMode,
    step: isize,
) -> crate::app_config::AntialiasingMode {
    crate::app_config::cycle_antialiasing_mode(current, step)
}
