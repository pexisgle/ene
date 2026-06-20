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
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label("Character");
            if ui.button("<").clicked() {
                // PR9: cycle to the previous character and
                // push the new character's per-character
                // default expression so the renderer
                // immediately picks it up. The
                // `select_character` dispatch is a no-op when
                // the cycle is a same-character request, so
                // we only push on an actual switch.
                let len = settings.characters.len();
                if len > 0 {
                    let idx = ((settings.character_state.selected_character as isize - 1)
                        .rem_euclid(len as isize)) as usize;
                    if let Some(default_expression) = settings.select_character(idx) {
                        emotion_queue.push(EmotionCommand {
                            emotion: default_expression,
                            target_time: now_secs,
                            hold_secs: 4.0,
                            weight: 1.0,
                        });
                    }
                }
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(format_character_label(settings)),
            );
            if ui.button(">").clicked() {
                let len = settings.characters.len();
                if len > 0 {
                    let idx = ((settings.character_state.selected_character as isize + 1)
                        .rem_euclid(len as isize)) as usize;
                    if let Some(default_expression) = settings.select_character(idx) {
                        emotion_queue.push(EmotionCommand {
                            emotion: default_expression,
                            target_time: now_secs,
                            hold_secs: 4.0,
                            weight: 1.0,
                        });
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Motion");
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::PrevMotion,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
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
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label("Animation");
            if ui.button("Toggle").clicked() {
                apply_action(
                    SettingsAction::TogglePlay,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                );
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

        render_linux_only(ui, settings, animation, ai, world, ui_entity);

        // PR5.6: per-bone collider wireframe + raycast
        // hit-point overlay toggle. The F3 hotkey is the
        // keyboard equivalent.
        ui.horizontal(|ui| {
            ui.label("Raycast Colliders (Debug)");
            let debug_on = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                ui_state.show_collider_debug
            } else {
                false
            };
            let mut checkbox = debug_on;
            if ui.checkbox(&mut checkbox, "").changed() && checkbox != debug_on {
                apply_action(
                    SettingsAction::ToggleColliderDebug,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(if checkbox {
                    "Visible (F3)"
                } else {
                    "Hidden (F3)"
                }),
            );
        });

        let debug_on = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
            ui_state.show_collider_debug
        } else {
            false
        };
        if debug_on {
            ui.horizontal(|ui| {
                ui.label("Hovered Bone");
                let name = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                    ui_state.hovered_bone_name.clone()
                } else {
                    None
                };
                ui.label(name.as_deref().unwrap_or("None"));
            });
        }

        render_numeric_row(
            ui,
            "LookAt Strength",
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
            |s, buf| *buf = format!("{:.2}", s.character_state.model_scale),
            |s, v| s.character_state.model_scale = v,
            world,
            ui_entity,
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
            |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.x),
            |s, v| s.character_state.character_position.x = v,
            world,
            ui_entity,
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
            |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.y),
            |s, v| s.character_state.character_position.y = v,
            world,
            ui_entity,
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
            |s, buf| *buf = format!("{:+.2}", s.character_state.character_position.z),
            |s, v| s.character_state.character_position.z = v,
            world,
            ui_entity,
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
                        weight: 1.0,
                    });
                }
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
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
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) where
    F: Fn(&CharacterSettings, &mut String),
    C: Fn(&mut CharacterSettings, f32),
{
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.button("-").clicked() {
            apply_action(down, settings, animation, ai, world, ui_entity);
            // PR4.2 follow-up: re-derive the textbox content from
            // the new settings value so the displayed number stays
            // in sync with the +/- button input. The legacy Bevy
            // code did the same re-format on every dispatch.
            refresh(settings, buffer);
        }
        let response = ui.add(egui::TextEdit::singleline(buffer).desired_width(220.0));
        // PR2.1: keyboard re-parse on Enter / focus loss.
        // The legacy Bevy code re-parsed `TextEdit::singleline`
        // on Enter and on focus loss. The +/- buttons are still
        // the primary input (the buffer is auto-refreshed from
        // the settings on every +/- click) but typing a value
        // and pressing Enter (or tabbing out) now commits.
        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter_pressed {
            if let Ok(value) = buffer.trim().parse::<f32>() {
                commit(settings, value);
                settings.mark_dirty();
                settings.clamp_runtime_values();
                refresh(settings, buffer);
            } else {
                // Reject: revert the buffer to the live value.
                refresh(settings, buffer);
            }
        } else if response.lost_focus() {
            // Tab/click-away without Enter: also commit (parses
            // whatever the user typed), then re-format from
            // the settings so the displayed text is canonical.
            if let Ok(value) = buffer.trim().parse::<f32>() {
                commit(settings, value);
                settings.mark_dirty();
                settings.clamp_runtime_values();
            }
            refresh(settings, buffer);
        }
        if ui.button("+").clicked() {
            apply_action(up, settings, animation, ai, world, ui_entity);
            refresh(settings, buffer);
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
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    ui.horizontal(|ui| {
        ui.label("Debug Overlay");
        if ui.button("Toggle").clicked() {
            apply_action(
                SettingsAction::ToggleDebugOverlay,
                settings,
                animation,
                ai,
                world,
                ui_entity,
            );
        }
        let debug_overlay_visible =
            if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                ui_state.debug_overlay_visible
            } else {
                false
            };
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(if debug_overlay_visible {
                "Visible"
            } else {
                "Hidden"
            }),
        );
    });

    ui.horizontal(|ui| {
        ui.label("Mask Downsample");
        if ui.button("<").clicked() {
            apply_action(
                SettingsAction::MaskDownsampleDown,
                settings,
                animation,
                ai,
                world,
                ui_entity,
            );
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(format!("{}x", settings.graphics.mask_render_downsample)),
        );
        if ui.button(">").clicked() {
            apply_action(
                SettingsAction::MaskDownsampleUp,
                settings,
                animation,
                ai,
                world,
                ui_entity,
            );
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn render_linux_only(
    _ui: &mut egui::Ui,
    _settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    _ai: &Arc<AiBridge>,
    _world: &mut hecs::World,
    _ui_entity: hecs::Entity,
) {
}
