//! Character settings page.
//!
//! Cycle rows for character and motion selection, a toggle for
//! animation play/pause, four numeric rows for the look-at / scale
//! / X / Y / Z parameters, six manual expression-test buttons, and
//! (Linux only) the debug overlay + mask downsample cycle rows.
use super::input::SettingsInputState;
use super::widgets::{SettingsAction, apply_action};
use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionCommand, EmotionQueue};
use crate::settings::CharacterSettings;
use std::sync::Arc;

const EXPRESSIONS: [&str; 6] = ["happy", "sad", "angry", "relaxed", "surprised", "neutral"];

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    emotion_queue: &mut EmotionQueue,
    now_secs: f64,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label("Character");
            if ui.button("<").clicked() {
                apply_action(SettingsAction::PrevCharacter, settings, animation, ai);
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_character_label(settings)),
            );
            if ui.button(">").clicked() {
                apply_action(SettingsAction::NextCharacter, settings, animation, ai);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Motion");
            if ui.button("<").clicked() {
                apply_action(SettingsAction::PrevMotion, settings, animation, ai);
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_motion_label(settings)),
            );
            if ui.button(">").clicked() {
                apply_action(SettingsAction::NextMotion, settings, animation, ai);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Animation");
            if ui.button("Toggle").clicked() {
                apply_action(SettingsAction::TogglePlay, settings, animation, ai);
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(if animation.playing {
                    "Playing"
                } else {
                    "Paused"
                }),
            );
        });

        render_linux_only(ui, settings, animation, ai);

        render_numeric_row(
            ui,
            "LookAt Strength",
            &mut input.look_at_strength,
            settings,
            animation,
            ai,
            SettingsAction::LookAtStrengthDown,
            SettingsAction::LookAtStrengthUp,
        );
        render_numeric_row(
            ui,
            "Model Scale",
            &mut input.model_scale,
            settings,
            animation,
            ai,
            SettingsAction::ModelScaleDown,
            SettingsAction::ModelScaleUp,
        );
        render_numeric_row(
            ui,
            "Character Pos X",
            &mut input.character_pos_x,
            settings,
            animation,
            ai,
            SettingsAction::CharacterPosXDown,
            SettingsAction::CharacterPosXUp,
        );
        render_numeric_row(
            ui,
            "Character Pos Y",
            &mut input.character_pos_y,
            settings,
            animation,
            ai,
            SettingsAction::CharacterPosYDown,
            SettingsAction::CharacterPosYUp,
        );
        render_numeric_row(
            ui,
            "Character Pos Z",
            &mut input.character_pos_z,
            settings,
            animation,
            ai,
            SettingsAction::CharacterPosZDown,
            SettingsAction::CharacterPosZUp,
        );

        ui.separator();
        ui.label("Manual Expressions (Test)");
        ui.horizontal(|ui| {
            for emotion in EXPRESSIONS {
                if ui.button(emotion).clicked() {
                    emotion_queue.push(EmotionCommand {
                        emotion: emotion.to_string(),
                        target_time: now_secs,
                        hold_secs: 4.0,
                    });
                }
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn render_numeric_row(
    ui: &mut egui::Ui,
    label: &str,
    buffer: &mut String,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    down: SettingsAction,
    up: SettingsAction,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.button("-").clicked() {
            apply_action(down, settings, animation, ai);
        }
        let _response = ui.add(egui::TextEdit::singleline(buffer).desired_width(220.0));
        if ui.button("+").clicked() {
            apply_action(up, settings, animation, ai);
        }
    });
}

fn format_character_label(settings: &CharacterSettings) -> String {
    format!(
        "[{}/{}] {}",
        settings.character_state.selected_character + 1,
        settings.characters.len(),
        settings.current_entry().name
    )
}

fn format_motion_label(settings: &CharacterSettings) -> String {
    let entry = settings.current_entry();
    let name = entry
        .motion_names
        .get(settings.character_state.selected_motion)
        .cloned()
        .unwrap_or_else(|| compact_asset_name(settings.current_motion()));
    format!(
        "[{}/{}] {}",
        settings.character_state.selected_motion + 1,
        entry.motion_names.len(),
        name
    )
}

fn compact_asset_name(path: &str) -> String {
    if path.len() <= 30 {
        return path.to_string();
    }
    format!("...{}", &path[path.len() - 27..])
}

#[cfg(target_os = "linux")]
fn render_linux_only(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
) {
    ui.horizontal(|ui| {
        ui.label("Debug Overlay");
        if ui.button("Toggle").clicked() {
            apply_action(SettingsAction::ToggleDebugOverlay, settings, animation, ai);
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(if settings.ui.debug_overlay_visible {
                "Visible"
            } else {
                "Hidden"
            }),
        );
    });

    ui.horizontal(|ui| {
        ui.label("Mask Downsample");
        if ui.button("<").clicked() {
            apply_action(SettingsAction::MaskDownsampleDown, settings, animation, ai);
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(format!("{}x", settings.graphics.mask_render_downsample)),
        );
        if ui.button(">").clicked() {
            apply_action(SettingsAction::MaskDownsampleUp, settings, animation, ai);
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn render_linux_only(
    _ui: &mut egui::Ui,
    _settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    _ai: &Arc<AiBridge>,
) {
}
