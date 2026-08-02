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
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
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
    world: &mut World,
    ui_entity: Entity,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "character"));
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::PrevCharacter,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    Some(emotion_queue),
                    now_secs,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_character_label(settings)),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::NextCharacter,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    Some(emotion_queue),
                    now_secs,
                );
            }
        });

        render_asset_warnings(ui, settings);

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "motion"));
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::PrevMotion,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    now_secs,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_motion_label(settings)),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::NextMotion,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    now_secs,
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "animation"));
            if ui.button("Toggle").clicked() {
                apply_action(
                    SettingsAction::TogglePlay,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    now_secs,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(if animation.playing {
                    i18n_embed_fl::fl!(crate::i18n::loader(), "playing")
                } else {
                    i18n_embed_fl::fl!(crate::i18n::loader(), "paused")
                }),
            );
        });

        render_numeric_row(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "lookat-strength"),
            &mut input.look_at_strength,
            settings,
            animation,
            ai,
            SettingsAction::LookAtStrengthDown,
            SettingsAction::LookAtStrengthUp,
            |s, buf| *buf = format!("{:.2}", s.character_state.look_at_strength),
            |s, v| s.character_state.look_at_strength = v,
            world,
            ui_entity,
            now_secs,
        );
        render_numeric_row(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "model-scale"),
            &mut input.model_scale,
            settings,
            animation,
            ai,
            SettingsAction::ModelScaleDown,
            SettingsAction::ModelScaleUp,
            |s, buf| *buf = format!("{:.2}", s.character_state.model_scale),
            |s, v| s.character_state.model_scale = v,
            world,
            ui_entity,
            now_secs,
        );
        render_numeric_row(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "character-pos-x"),
            &mut input.character_pos_x,
            settings,
            animation,
            ai,
            SettingsAction::CharacterPosXDown,
            SettingsAction::CharacterPosXUp,
            |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.x),
            |s, v| s.character_state.character_position.x = v,
            world,
            ui_entity,
            now_secs,
        );
        render_numeric_row(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "character-pos-y"),
            &mut input.character_pos_y,
            settings,
            animation,
            ai,
            SettingsAction::CharacterPosYDown,
            SettingsAction::CharacterPosYUp,
            |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.y),
            |s, v| s.character_state.character_position.y = v,
            world,
            ui_entity,
            now_secs,
        );
        render_numeric_row(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "character-pos-z"),
            &mut input.character_pos_z,
            settings,
            animation,
            ai,
            SettingsAction::CharacterPosZDown,
            SettingsAction::CharacterPosZUp,
            |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.z),
            |s, v| s.character_state.character_position.z = v,
            world,
            ui_entity,
            now_secs,
        );

        ui.horizontal(|ui| {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "reset-position"))
                .clicked()
            {
                apply_action(
                    SettingsAction::ResetCharacterPosition,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    now_secs,
                );
                // Mirror the reset into the editable buffers so the
                // X/Y/Z text fields immediately show "+0.00".
                input.character_pos_x =
                    format!("{:+.2}", settings.character_state.character_position.x);
                input.character_pos_y =
                    format!("{:+.2}", settings.character_state.character_position.y);
                input.character_pos_z =
                    format!("{:+.2}", settings.character_state.character_position.z);
            }
        });

        ui.separator();
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "manual-expressions"
        ));
        ui.horizontal(|ui| {
            for emotion in EXPRESSIONS {
                if ui.button(emotion).clicked() {
                    emotion_queue.push(EmotionCommand {
                        emotion: emotion.to_string(),
                        target_time: now_secs,
                        hold_secs: 4.0,
                        weight: 1.0,
                    });
                }
            }
        });
    });
}

fn render_numeric_row<F, C>(
    ui: &mut egui::Ui,
    label: &str,
    buffer: &mut String,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    down: SettingsAction,
    up: SettingsAction,
    refresh: F,
    commit: C,
    world: &mut World,
    ui_entity: Entity,
    now_secs: f64,
) where
    F: Fn(&CharacterSettings, &mut String),
    C: Fn(&mut CharacterSettings, f32),
{
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.button("-").clicked() {
            apply_action(
                down, settings, animation, ai, world, ui_entity, None, now_secs,
            );
            refresh(settings, buffer);
        }
        let response = ui.add(egui::TextEdit::singleline(buffer).desired_width(220.0));
        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter_pressed {
            if let Ok(value) = buffer.trim().parse::<f32>() {
                commit(settings, value);
                settings.mark_dirty();
                settings.clamp_runtime_values();
                refresh(settings, buffer);
            } else {
                refresh(settings, buffer);
            }
        } else if response.lost_focus() {
            if let Ok(value) = buffer.trim().parse::<f32>() {
                commit(settings, value);
                settings.mark_dirty();
                settings.clamp_runtime_values();
            }
            refresh(settings, buffer);
        }
        if ui.button("+").clicked() {
            apply_action(
                up, settings, animation, ai, world, ui_entity, None, now_secs,
            );
            refresh(settings, buffer);
        }
        if !response.has_focus() {
            refresh(settings, buffer);
        }
    });
}

fn render_asset_warnings(ui: &mut egui::Ui, settings: &CharacterSettings) {
    let Some(entry) = settings.current_entry() else {
        ui.colored_label(
            egui::Color32::from_rgb(220, 160, 60),
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-none-selected"),
        );
        return;
    };
    if entry.vrm_paths.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 160, 60),
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-missing-vrm"),
        );
    }
    if entry.motion_paths.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 160, 60),
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-missing-motion"),
        );
    }
}

fn format_character_label(settings: &CharacterSettings) -> String {
    let total = settings.characters.len();
    let (index, name) = settings.current_entry().map_or((0, "—"), |entry| {
        (
            settings.character_state.selected_character + 1,
            entry.name.as_str(),
        )
    });
    format!("[{index}/{total}] {name}")
}

fn format_motion_label(settings: &CharacterSettings) -> String {
    let Some(entry) = settings.current_entry() else {
        return "[0/0] —".to_string();
    };
    let total = entry.motion_names.len();
    let name = entry
        .motion_names
        .get(settings.character_state.selected_motion)
        .cloned()
        .or_else(|| settings.current_motion().map(compact_asset_name))
        .unwrap_or_else(|| "—".to_string());
    let index = if total == 0 {
        0
    } else {
        settings.character_state.selected_motion + 1
    };
    format!("[{index}/{total}] {name}")
}

fn compact_asset_name(path: &str) -> String {
    if path.len() <= 30 {
        return path.to_string();
    }
    format!("...{}", &path[path.len() - 27..])
}
