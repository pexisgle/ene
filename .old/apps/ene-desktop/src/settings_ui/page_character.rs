//! Numeric rows keep the commit-on-focus-loss contract of the editable
//! buffers.
use super::components::{BadgeTone, section_card, setting_row, status_badge, warning_box};
use super::input::SettingsInputState;
use super::widgets::{SettingsAction, apply_action};
use crate::character_state::{AnimationControl, EmotionCommand, EmotionQueue};
use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::ops::RangeInclusive;
use std::sync::Arc;

const EXPRESSIONS: [&str; 6] = ["happy", "sad", "angry", "relaxed", "surprised", "neutral"];

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    emotion_queue: &mut EmotionQueue,
    now_secs: f64,
    world: &mut World,
    ui_entity: Entity,
) {
    ui.vertical(|ui| {
        section_card(
            ui,
            "character-model",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-character-model"),
            |ui| {
                render_asset_warnings(ui, settings);
                render_character_selector(
                    ui,
                    settings,
                    animation,
                    ai,
                    emotion_queue,
                    now_secs,
                    world,
                    ui_entity,
                );
                render_motion_selector(ui, settings, animation, ai, now_secs, world, ui_entity);
                setting_row(
                    ui,
                    "character_animation_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "animation"),
                    "",
                    |ui| {
                        if ui
                            .button(i18n_embed_fl::fl!(crate::i18n::loader(), "toggle"))
                            .clicked()
                        {
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
                        status_badge(
                            ui,
                            &if animation.playing {
                                i18n_embed_fl::fl!(crate::i18n::loader(), "playing")
                            } else {
                                i18n_embed_fl::fl!(crate::i18n::loader(), "paused")
                            },
                            if animation.playing {
                                BadgeTone::Ok
                            } else {
                                BadgeTone::Neutral
                            },
                        );
                    },
                );
            },
        );

        section_card(
            ui,
            "character-transform",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-character-transform"),
            |ui| {
                render_numeric_row(
                    ui,
                    "look_at_strength",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "lookat-strength"),
                    &mut input.look_at_strength,
                    0.0..=1.0,
                    settings,
                    animation,
                    ai,
                    SettingsAction::LookAtStrengthDown,
                    SettingsAction::LookAtStrengthUp,
                    |s| s.character_state.look_at_strength,
                    |s, buf| *buf = format!("{:.2}", s.character_state.look_at_strength),
                    |s, v| s.character_state.look_at_strength = v,
                    world,
                    ui_entity,
                    now_secs,
                );
                render_numeric_row(
                    ui,
                    "model_scale",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "model-scale"),
                    &mut input.model_scale,
                    0.25..=4.0,
                    settings,
                    animation,
                    ai,
                    SettingsAction::ModelScaleDown,
                    SettingsAction::ModelScaleUp,
                    |s| s.character_state.model_scale,
                    |s, buf| *buf = format!("{:.2}", s.character_state.model_scale),
                    |s, v| s.character_state.model_scale = v,
                    world,
                    ui_entity,
                    now_secs,
                );
                render_numeric_row(
                    ui,
                    "character_pos_x",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "character-pos-x"),
                    &mut input.character_pos_x,
                    -3.0..=3.0,
                    settings,
                    animation,
                    ai,
                    SettingsAction::CharacterPosXDown,
                    SettingsAction::CharacterPosXUp,
                    |s| s.character_state.character_position.x,
                    |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.x),
                    |s, v| s.character_state.character_position.x = v,
                    world,
                    ui_entity,
                    now_secs,
                );
                render_numeric_row(
                    ui,
                    "character_pos_y",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "character-pos-y"),
                    &mut input.character_pos_y,
                    -2.0..=3.0,
                    settings,
                    animation,
                    ai,
                    SettingsAction::CharacterPosYDown,
                    SettingsAction::CharacterPosYUp,
                    |s| s.character_state.character_position.y,
                    |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.y),
                    |s, v| s.character_state.character_position.y = v,
                    world,
                    ui_entity,
                    now_secs,
                );
                render_numeric_row(
                    ui,
                    "character_pos_z",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "character-pos-z"),
                    &mut input.character_pos_z,
                    -4.0..=3.0,
                    settings,
                    animation,
                    ai,
                    SettingsAction::CharacterPosZDown,
                    SettingsAction::CharacterPosZUp,
                    |s| s.character_state.character_position.z,
                    |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.z),
                    |s, v| s.character_state.character_position.z = v,
                    world,
                    ui_entity,
                    now_secs,
                );
                ui.add_space(4.0);
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
            },
        );

        section_card(
            ui,
            "character-expressions",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-character-expressions"),
            |ui| {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "manual-expressions"
                ));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
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
            },
        );
    });
}

fn render_character_selector(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<CoreSession>,
    emotion_queue: &mut EmotionQueue,
    now_secs: f64,
    world: &mut World,
    ui_entity: Entity,
) {
    let selected = settings.character_state.selected_character;
    let mut direct_selection = selected;
    let options = settings
        .characters
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                index,
                format!(
                    "[{}/{}] {}",
                    index + 1,
                    settings.characters.len(),
                    entry.name
                ),
            )
        })
        .collect::<Vec<_>>();
    let mut action = None;
    setting_row(
        ui,
        "character_selector",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "character"),
        "",
        |ui| {
            if ui.button("‹").clicked() {
                action = Some(SettingsAction::PrevCharacter);
            }
            let combo_width = (ui.available_width() - 72.0).clamp(140.0, 280.0);
            egui::ComboBox::from_id_salt("character_combo")
                .selected_text(format_character_label(settings))
                .width(combo_width)
                .show_ui(ui, |ui| {
                    for (index, label) in &options {
                        ui.selectable_value(&mut direct_selection, *index, label);
                    }
                });
            if direct_selection != selected {
                action = Some(SettingsAction::SelectCharacter(direct_selection));
            }
            if ui.button("›").clicked() {
                action = Some(SettingsAction::NextCharacter);
            }
        },
    );
    if let Some(action) = action {
        apply_action(
            action,
            settings,
            animation,
            ai,
            world,
            ui_entity,
            Some(emotion_queue),
            now_secs,
        );
    }
}

fn render_motion_selector(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<CoreSession>,
    now_secs: f64,
    world: &mut World,
    ui_entity: Entity,
) {
    let selected = settings.character_state.selected_motion;
    let mut direct_selection = selected;
    let options = settings.current_entry().map_or_else(Vec::new, |entry| {
        entry
            .motion_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    index,
                    format!("[{}/{}] {name}", index + 1, entry.motion_names.len()),
                )
            })
            .collect::<Vec<_>>()
    });
    let mut action = None;
    setting_row(
        ui,
        "motion_selector",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "motion"),
        "",
        |ui| {
            if ui.button("‹").clicked() {
                action = Some(SettingsAction::PrevMotion);
            }
            let combo_width = (ui.available_width() - 72.0).clamp(140.0, 280.0);
            egui::ComboBox::from_id_salt("motion_combo")
                .selected_text(format_motion_label(settings))
                .width(combo_width)
                .show_ui(ui, |ui| {
                    for (index, label) in &options {
                        ui.selectable_value(&mut direct_selection, *index, label);
                    }
                });
            if direct_selection != selected {
                action = Some(SettingsAction::SelectMotion(direct_selection));
            }
            if ui.button("›").clicked() {
                action = Some(SettingsAction::NextMotion);
            }
        },
    );
    if let Some(action) = action {
        apply_action(
            action, settings, animation, ai, world, ui_entity, None, now_secs,
        );
    }
}

fn render_numeric_row<G, F, C>(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: &str,
    buffer: &mut String,
    range: RangeInclusive<f32>,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<CoreSession>,
    down: SettingsAction,
    up: SettingsAction,
    current: G,
    refresh: F,
    commit: C,
    world: &mut World,
    ui_entity: Entity,
    now_secs: f64,
) where
    G: Fn(&CharacterSettings) -> f32,
    F: Fn(&CharacterSettings, &mut String),
    C: Fn(&mut CharacterSettings, f32),
{
    setting_row(ui, id_salt, label, "", |ui| {
        if ui.button("-").clicked() {
            apply_action(
                down, settings, animation, ai, world, ui_entity, None, now_secs,
            );
            refresh(settings, buffer);
        }
        let mut slider_value = current(settings);
        let slider_width = (ui.available_width() - 142.0).clamp(120.0, 220.0);
        let slider_changed = ui
            .add_sized(
                [slider_width, 0.0],
                egui::Slider::new(&mut slider_value, range)
                    .step_by(0.05)
                    .show_value(false),
            )
            .changed();
        if slider_changed {
            commit(settings, slider_value);
            settings.clamp_runtime_values();
            settings.mark_dirty();
            refresh(settings, buffer);
        }
        let response = ui.add(egui::TextEdit::singleline(buffer).desired_width(72.0));
        if response.lost_focus() {
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
        if !response.has_focus() && !slider_changed {
            refresh(settings, buffer);
        }
    });
}

fn render_asset_warnings(ui: &mut egui::Ui, settings: &CharacterSettings) {
    let Some(entry) = settings.current_entry() else {
        warning_box(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-none-selected"),
        );
        return;
    };
    if entry.vrm_paths.is_empty() {
        warning_box(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-missing-vrm"),
        );
    }
    if entry.motion_paths.is_empty() {
        warning_box(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-missing-motion"),
        );
    }
}

fn format_character_label(settings: &CharacterSettings) -> String {
    let total = settings.characters.len();
    match settings.current_entry() {
        Some(entry) => format!(
            "[{}/{}] {}",
            settings.character_state.selected_character + 1,
            total,
            entry.name
        ),
        None => {
            // Only reachable when every entry was removed; an out-of-range
            // index is clamped before this point, so total > 0 with no entry
            // is dead.
            if total == 0 {
                "[0/0] —".to_string()
            } else {
                format!("[0/{total}] —")
            }
        }
    }
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
