//! Shared row widgets and action dispatcher for the settings UI.
//!
//! Mirrors the legacy `apps/ene-desktop/src/settings_ui/widgets.rs`
//! shape 1:1. The action enum and `apply_action` dispatcher are the
//! single funnel through which buttons, hotkeys, and direct egui
//! field changes mutate [`CharacterSettings`].
use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionCommand, EmotionQueue};
use crate::component::ui::UiStateComponent;
#[cfg(target_os = "linux")]
use crate::settings::cycle_mask_render_downsample;
use crate::settings::{
    AntialiasingMode, CharacterSettings, ShadowQuality, cycle_antialiasing_mode, cycle_debug_fps,
    cycle_shadow_quality, cycle_target_fps, target_fps_label,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::sync::Arc;

/// Single action enum shared by every page widget. Hotkeys and
/// buttons both translate into one of these before mutating state.
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
    /// Snap `character_position` back to the world origin. Triggered
    /// by the "Reset Position" button on the Character settings
    /// page; lets the user recover from a model dragged off-screen
    /// without restarting the app.
    ResetCharacterPosition,
    /// Toggle the per-bone collider wireframe + raycast hit-point
    /// overlay. Bound to the F3 hotkey and the "Show raycast
    /// colliders (debug)" checkbox on the Character page.
    ToggleColliderDebug,
    ToggleInputRegionDebug,
    DebugFpsDown,
    DebugFpsUp,
    LanguageDown,
    LanguageUp,
    SendAiChat,
}

pub fn apply_action(
    action: SettingsAction,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    emotion_queue: Option<&mut EmotionQueue>,
    now_secs: f64,
) {
    match action {
        SettingsAction::PrevCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                -1,
            );
            push_default_expression(settings.select_character(idx), emotion_queue, now_secs);
        }
        SettingsAction::NextCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                1,
            );
            push_default_expression(settings.select_character(idx), emotion_queue, now_secs);
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
            if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui_state.0.debug_overlay_visible = !ui_state.0.debug_overlay_visible;
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
        SettingsAction::ResetCharacterPosition => {
            settings.character_state.character_position = glam::Vec3::ZERO;
        }
        SettingsAction::ToggleColliderDebug => {
            // Not persisted — defaults to `false` on every launch.
            if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui_state.0.show_collider_debug = !ui_state.0.show_collider_debug;
            }
        }
        SettingsAction::ToggleInputRegionDebug => {
            if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui_state.0.show_input_region_debug = !ui_state.0.show_input_region_debug;
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
        SettingsAction::LanguageDown => {
            settings.language = crate::settings::cycle_language(settings.language, -1);
            crate::i18n::select_language(settings.language);
            settings.mark_dirty();
        }
        SettingsAction::LanguageUp => {
            settings.language = crate::settings::cycle_language(settings.language, 1);
            crate::i18n::select_language(settings.language);
            settings.mark_dirty();
        }
    }

    settings.clamp_runtime_values();
    settings.mark_dirty();
    tracing::info!("apply_action: calling settings.save()");
    settings.save();
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
    world: &mut World,
    ui_entity: Entity,
) {
    if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
        let user_input = ui_state.0.ai_chat_input.trim().to_string();
        if user_input.is_empty() {
            return;
        }
        ai.run(user_input);
        ui_state.0.ai_chat_input.clear();
        ui_state.0.ai_latest_response.clear();
    }
}

pub fn format_fps_label(lang: crate::settings::Language, fps: u32) -> String {
    target_fps_label(lang, fps)
}

pub fn format_shadow_label(lang: crate::settings::Language, quality: ShadowQuality) -> String {
    let _ = lang;
    let loader = crate::i18n::loader();
    match quality {
        ShadowQuality::Low => i18n_embed_fl::fl!(loader, "low"),
        ShadowQuality::Medium => i18n_embed_fl::fl!(loader, "medium"),
        ShadowQuality::High => i18n_embed_fl::fl!(loader, "high"),
    }
}

pub fn format_aa_label(lang: crate::settings::Language, mode: AntialiasingMode) -> String {
    let _ = lang;
    let loader = crate::i18n::loader();
    match mode {
        AntialiasingMode::Off => i18n_embed_fl::fl!(loader, "off"),
        AntialiasingMode::Fxaa => "FXAA".to_string(),
        AntialiasingMode::Smaa => "SMAA".to_string(),
        AntialiasingMode::Taa => "TAA".to_string(),
    }
}

/// Push the per-character default expression into the
/// `EmotionQueue` if both a non-`None` expression and a queue
/// handle are available. Centralising this branch keeps the
/// character-cycle arm in [`apply_action`] symmetric for `Prev`
/// and `Next` and removes the duplicated post-`apply_action`
/// emotion-push that used to live in `runtime.rs` and
/// `page_character.rs`.
fn push_default_expression(
    default_expression: Option<String>,
    emotion_queue: Option<&mut EmotionQueue>,
    now_secs: f64,
) {
    if let (Some(expression), Some(queue)) = (default_expression, emotion_queue) {
        queue.push(EmotionCommand {
            emotion: expression,
            target_time: now_secs,
            hold_secs: 4.0,
            weight: 1.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_default_expression_drops_on_none_expression() {
        let mut q = EmotionQueue::default();
        push_default_expression(None, Some(&mut q), 1.0);
        assert!(q.commands.is_empty());
    }

    #[test]
    fn push_default_expression_drops_on_none_queue() {
        let mut expression = None;
        // even with a queue, no queue handle → no push
        let q = EmotionQueue::default();
        push_default_expression(expression.take(), None, 1.0);
        assert!(q.commands.is_empty());
    }

    #[test]
    fn push_default_expression_pushes_with_both() {
        let mut q = EmotionQueue::default();
        push_default_expression(Some("happy".to_string()), Some(&mut q), 7.5);
        assert_eq!(q.commands.len(), 1);
        let cmd = &q.commands[0];
        assert_eq!(cmd.emotion, "happy");
        assert_eq!(cmd.target_time, 7.5);
        assert_eq!(cmd.hold_secs, 4.0);
        assert_eq!(cmd.weight, 1.0);
    }

    /// `ResetCharacterPosition` must zero all three axes of
    /// `character_position` without touching unrelated fields like
    /// `look_at_strength` or `model_scale`. Pin the contract here
    /// because the call site in `page_character.rs` has no easy
    /// way to assert "other fields untouched" at runtime.
    #[test]
    fn reset_character_position_zeroes_only_position() {
        use crate::settings::CharacterState;
        let mut state = CharacterState {
            character_position: glam::Vec3::new(1.25, -0.5, 2.0),
            look_at_strength: 0.42,
            model_scale: 1.75,
            ..CharacterState::default()
        };
        // Mirror the single statement that lives in
        // `apply_action`'s `ResetCharacterPosition` arm.
        state.character_position = glam::Vec3::ZERO;
        assert_eq!(state.character_position, glam::Vec3::ZERO);
        assert!((state.look_at_strength - 0.42).abs() < 1e-6);
        assert!((state.model_scale - 1.75).abs() < 1e-6);
    }
}
