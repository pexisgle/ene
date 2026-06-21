//! Shared row widgets and action dispatcher for the settings UI.
//!
//! Mirrors the legacy `apps/ene-desktop/src/settings_ui/widgets.rs`
//! shape 1:1. The action enum and `apply_action` dispatcher are the
//! single funnel through which buttons, hotkeys, and direct egui
//! field changes mutate [`CharacterSettings`].
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
#[cfg(target_os = "linux")]
use crate::settings::cycle_mask_render_downsample;
use crate::settings::{
    AntialiasingMode, CharacterSettings, ShadowQuality, cycle_antialiasing_mode, cycle_debug_fps,
    cycle_shadow_quality, cycle_target_fps, target_fps_label,
};
use std::sync::Arc;

/// Single action enum shared by every page widget. Hotkeys and
/// buttons both translate into one of these before mutating state.
#[allow(dead_code)] // Every variant is dispatched by `apply_action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    PrevCharacter,
    NextCharacter,
    PrevMotion,
    NextMotion,
    TogglePlay,
    #[cfg(target_os = "linux")]
    ToggleDebugOverlay,
    #[cfg(target_os = "linux")]
    MaskDownsampleDown,
    #[cfg(target_os = "linux")]
    MaskDownsampleUp,
    TargetFpsDown,
    TargetFpsUp,
    ShadowQualityDown,
    ShadowQualityUp,
    AntialiasingModeDown,
    AntialiasingModeUp,
    LookAtStrengthDown,
    LookAtStrengthUp,
    ModelScaleDown,
    ModelScaleUp,
    CharacterPosXDown,
    CharacterPosXUp,
    CharacterPosYDown,
    CharacterPosYUp,
    CharacterPosZDown,
    CharacterPosZUp,
    /// Toggle the per-bone collider wireframe + raycast hit-point
    /// overlay. Bound to the F3 hotkey and the "Show raycast
    /// colliders (debug)" checkbox on the Character page.
    ToggleColliderDebug,
    ToggleInputRegionDebug,
    DebugFpsDown,
    DebugFpsUp,
    SendAiChat,
}

pub fn apply_action(
    action: SettingsAction,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    match action {
        SettingsAction::PrevCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                -1,
            );
            // `select_character` returns the per-character
            // default expression; the page_character / WASD
            // hotkey paths are responsible for pushing it into
            // the renderer's `EmotionQueue`.
            settings.select_character(idx);
        }
        SettingsAction::NextCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                1,
            );
            settings.select_character(idx);
        }
        SettingsAction::PrevMotion => {
            settings.character_state.selected_motion = cycle_index(
                settings.character_state.selected_motion,
                settings.current_entry().motion_names.len(),
                -1,
            );
            settings.character_state.motion_override = None;
            settings.character_state.needs_respawn = true;
            settings.mark_dirty();
        }
        SettingsAction::NextMotion => {
            settings.character_state.selected_motion = cycle_index(
                settings.character_state.selected_motion,
                settings.current_entry().motion_names.len(),
                1,
            );
            settings.character_state.motion_override = None;
            settings.character_state.needs_respawn = true;
            settings.mark_dirty();
        }
        SettingsAction::TogglePlay => {
            animation.toggle_playing();
        }
        #[cfg(target_os = "linux")]
        SettingsAction::ToggleDebugOverlay => {
            if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
                ui_state.debug_overlay_visible = !ui_state.debug_overlay_visible;
            }
            settings.mark_dirty();
        }
        #[cfg(target_os = "linux")]
        SettingsAction::MaskDownsampleDown => {
            settings.graphics.mask_render_downsample =
                cycle_mask_render_downsample(settings.graphics.mask_render_downsample, -1);
            settings.mark_dirty();
        }
        #[cfg(target_os = "linux")]
        SettingsAction::MaskDownsampleUp => {
            settings.graphics.mask_render_downsample =
                cycle_mask_render_downsample(settings.graphics.mask_render_downsample, 1);
            settings.mark_dirty();
        }
        SettingsAction::TargetFpsDown => {
            settings.graphics.target_fps = cycle_target_fps(settings.graphics.target_fps, -1);
            settings.mark_dirty();
        }
        SettingsAction::TargetFpsUp => {
            settings.graphics.target_fps = cycle_target_fps(settings.graphics.target_fps, 1);
            settings.mark_dirty();
        }
        SettingsAction::ShadowQualityDown => {
            settings.graphics.shadow_quality =
                cycle_shadow_quality(settings.graphics.shadow_quality, -1);
            settings.mark_dirty();
        }
        SettingsAction::ShadowQualityUp => {
            settings.graphics.shadow_quality =
                cycle_shadow_quality(settings.graphics.shadow_quality, 1);
            settings.mark_dirty();
        }
        SettingsAction::AntialiasingModeDown => {
            settings.graphics.antialiasing_mode =
                cycle_antialiasing_mode(settings.graphics.antialiasing_mode, -1);
            settings.mark_dirty();
        }
        SettingsAction::AntialiasingModeUp => {
            settings.graphics.antialiasing_mode =
                cycle_antialiasing_mode(settings.graphics.antialiasing_mode, 1);
            settings.mark_dirty();
        }
        SettingsAction::LookAtStrengthDown => {
            adjust_f32(&mut settings.character_state.look_at_strength, -0.05);
        }
        SettingsAction::LookAtStrengthUp => {
            adjust_f32(&mut settings.character_state.look_at_strength, 0.05);
        }
        SettingsAction::ModelScaleDown => {
            adjust_f32(&mut settings.character_state.model_scale, -0.05);
        }
        SettingsAction::ModelScaleUp => {
            adjust_f32(&mut settings.character_state.model_scale, 0.05);
        }
        SettingsAction::CharacterPosXDown => {
            adjust_f32(&mut settings.character_state.character_position.x, -0.05);
        }
        SettingsAction::CharacterPosXUp => {
            adjust_f32(&mut settings.character_state.character_position.x, 0.05);
        }
        SettingsAction::CharacterPosYDown => {
            adjust_f32(&mut settings.character_state.character_position.y, -0.05);
        }
        SettingsAction::CharacterPosYUp => {
            adjust_f32(&mut settings.character_state.character_position.y, 0.05);
        }
        SettingsAction::CharacterPosZDown => {
            adjust_f32(&mut settings.character_state.character_position.z, -0.05);
        }
        SettingsAction::CharacterPosZUp => {
            adjust_f32(&mut settings.character_state.character_position.z, 0.05);
        }
        SettingsAction::ToggleColliderDebug => {
            // Not persisted — defaults to `false` on every launch.
            if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
                ui_state.show_collider_debug = !ui_state.show_collider_debug;
            }
        }
        SettingsAction::ToggleInputRegionDebug => {
            if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
                ui_state.show_input_region_debug = !ui_state.show_input_region_debug;
            }
        }
        SettingsAction::DebugFpsDown => {
            settings.graphics.debug_fps = cycle_debug_fps(settings.graphics.debug_fps, -1);
            settings.mark_dirty();
        }
        SettingsAction::DebugFpsUp => {
            settings.graphics.debug_fps = cycle_debug_fps(settings.graphics.debug_fps, 1);
            settings.mark_dirty();
        }
        SettingsAction::SendAiChat => {
            send_ai_chat(settings, ai, world, ui_entity);
        }
    }

    settings.clamp_runtime_values();
    settings.mark_dirty();
}

fn cycle_index(index: usize, len: usize, step: isize) -> usize {
    if len == 0 {
        return 0;
    }
    ((index as isize + step).rem_euclid(len as isize)) as usize
}

fn adjust_f32(value: &mut f32, delta: f32) {
    *value += delta;
}

fn send_ai_chat(
    _settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
        let user_input = ui_state.ai_chat_input.trim().to_string();
        if user_input.is_empty() {
            return;
        }
        ai.run(user_input);
        ui_state.ai_chat_input.clear();
        ui_state.ai_latest_response.clear();
    }
}

#[allow(dead_code)]
pub fn format_fps_label(fps: u32) -> String {
    target_fps_label(fps)
}

#[allow(dead_code)]
pub fn format_shadow_label(quality: ShadowQuality) -> &'static str {
    match quality {
        ShadowQuality::Low => "Low",
        ShadowQuality::Medium => "Medium",
        ShadowQuality::High => "High",
    }
}

#[allow(dead_code)]
pub fn format_aa_label(mode: AntialiasingMode) -> &'static str {
    match mode {
        AntialiasingMode::Off => "Off",
        AntialiasingMode::Fxaa => "Fxaa",
        AntialiasingMode::Smaa => "Smaa",
        AntialiasingMode::Taa => "Taa",
    }
}
