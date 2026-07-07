//! Debug settings page.
//!
//! Handles toggling raycast colliders, input region debug outlines,
//! throttling update rates via debug FPS, and (Linux only) the mask
//! overlay and downsampling factor.
use super::widgets::{SettingsAction, apply_action};
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::component::ui::UiStateComponent;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;
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
            ui.label(crate::i18n::raycast_colliders());
            let debug_on = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
                ui_state.0.show_collider_debug
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
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(if checkbox {
                    crate::i18n::visible_f3()
                } else {
                    crate::i18n::hidden_f3()
                }),
            );
        });

        let show_colliders = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
            ui_state.0.show_collider_debug
        } else {
            false
        };
        if show_colliders {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::hovered_bone());
                let name = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
                    ui_state.0.hovered_bone_name.clone()
                } else {
                    None
                };
                ui.label(name.unwrap_or_else(crate::i18n::none));
            });
        }

        ui.horizontal(|ui| {
            ui.label(crate::i18n::input_region_debug());
            let debug_on = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
                ui_state.0.show_input_region_debug
            } else {
                false
            };
            let mut checkbox = debug_on;
            if ui.checkbox(&mut checkbox, "").changed() && checkbox != debug_on {
                apply_action(
                    SettingsAction::ToggleInputRegionDebug,
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
                egui::Label::new(if checkbox {
                    crate::i18n::visible_f9()
                } else {
                    crate::i18n::hidden_f9()
                }),
            );
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::debug_update_fps());
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::DebugFpsDown,
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
                egui::Label::new(crate::settings::debug_fps_label(
                    settings.language,
                    settings.graphics.debug_fps,
                )),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::DebugFpsUp,
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

        render_linux_only(ui, settings, animation, ai, world, ui_entity);
        ui.separator();
        render_memory_journal(ui, ai, world, ui_entity);
    });
}

fn render_memory_journal(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    ui.heading(fl!(crate::i18n::loader(), "memory-journal-title"));
    let mut do_refresh = false;
    ui.horizontal(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "memory-journal-refresh"))
            .clicked()
        {
            do_refresh = true;
        }
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.checkbox(
                &mut state.0.memory_journal_show_deleted,
                fl!(crate::i18n::loader(), "memory-journal-show-deleted"),
            );
        }
    });

    if do_refresh {
        match ai.refresh_memory_journal(32) {
            Ok((mut memories, affect, commitments)) => {
                if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                    if !state.0.memory_journal_show_deleted {
                        memories.retain(|m| m.status != ene_memory::MemoryStatus::UserDeleted);
                    }
                    state.0.memory_journal_rows = memories
                        .into_iter()
                        .map(|m| crate::settings::MemoryJournalRow {
                            id: m.id.unwrap_or_default(),
                            title: m.title,
                            kind: m.kind.as_str().to_string(),
                            status: m.status.as_str().to_string(),
                            confidence: m.confidence.get(),
                            salience: m.salience.get(),
                            last_accessed: m.last_accessed_at.map(|ts| ts.to_rfc3339()),
                            why_recalled: format!(
                                "source={} access_count={}",
                                m.source.as_str(),
                                m.access_count
                            ),
                        })
                        .collect();
                    state.0.memory_journal_affect = format!(
                        "mood={} expression={} valence={:.2} arousal={:.2}",
                        affect.mood_label, affect.last_expression, affect.valence, affect.arousal
                    );
                    state.0.memory_journal_commitments = commitments
                        .into_iter()
                        .map(|c| format!("{} [{}]", c.title, c.status.as_str()))
                        .collect();
                    state.0.memory_journal_message =
                        Some(fl!(crate::i18n::loader(), "memory-journal-refresh-ok"));
                }
            }
            Err(error) => {
                if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                    state.0.memory_journal_message = Some(format!(
                        "{}: {error}",
                        fl!(crate::i18n::loader(), "memory-journal-refresh-error")
                    ));
                }
            }
        }
    }

    let snapshot = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone());
    let Some(snapshot) = snapshot else {
        return;
    };

    if let Some(message) = snapshot.memory_journal_message.as_deref() {
        ui.label(message);
    }
    if !snapshot.memory_journal_affect.is_empty() {
        ui.label(format!(
            "{}: {}",
            fl!(crate::i18n::loader(), "memory-journal-affect"),
            snapshot.memory_journal_affect
        ));
    }
    if !snapshot.memory_journal_commitments.is_empty() {
        ui.label(fl!(crate::i18n::loader(), "memory-journal-commitments"));
        for line in &snapshot.memory_journal_commitments {
            ui.label(format!("  - {line}"));
        }
    }

    egui::ScrollArea::vertical()
        .max_height(260.0)
        .show(ui, |ui| {
            if snapshot.memory_journal_rows.is_empty() {
                ui.weak(fl!(crate::i18n::loader(), "memory-journal-empty"));
                return;
            }
            for row in &snapshot.memory_journal_rows {
                ui.group(|ui| {
                    ui.label(format!(
                        "#{} [{}|{}] {}",
                        row.id, row.kind, row.status, row.title
                    ));
                    ui.label(format!(
                        "confidence={:.2} salience={:.2} last_accessed={}",
                        row.confidence,
                        row.salience,
                        row.last_accessed.as_deref().unwrap_or("-"),
                    ));
                    ui.label(format!("why: {}", row.why_recalled));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("pin").clicked() {
                            set_action_message(world, ui_entity, ai.pin_memory(row.id), "pin");
                        }
                        if ui.button("archive").clicked() {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(row.id, ene_memory::MemoryStatus::Archived),
                                "archive",
                            );
                        }
                        if ui.button("forget").clicked() {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(
                                    row.id,
                                    ene_memory::MemoryStatus::UserDeleted,
                                ),
                                "forget",
                            );
                        }
                        if ui.button("dispute").clicked() {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(row.id, ene_memory::MemoryStatus::Disputed),
                                "dispute",
                            );
                        }
                        if ui.button("restore").clicked() {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(row.id, ene_memory::MemoryStatus::Active),
                                "restore",
                            );
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
}

fn set_action_message(
    world: &mut World,
    ui_entity: Entity,
    result: Result<bool, String>,
    action: &str,
) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.memory_journal_message = Some(match result {
            Ok(true) => format!("memory {action} ok"),
            Ok(false) => format!("memory {action}: no change"),
            Err(error) => format!("memory {action} error: {error}"),
        });
    }
}

#[cfg(target_os = "linux")]
fn render_linux_only(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    ui.horizontal(|ui| {
        ui.label(crate::i18n::mask_overlay_debug());
        let debug_overlay_visible = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity)
        {
            ui_state.0.debug_overlay_visible
        } else {
            false
        };
        let mut checkbox = debug_overlay_visible;
        if ui.checkbox(&mut checkbox, "").changed() && checkbox != debug_overlay_visible {
            apply_action(
                SettingsAction::ToggleDebugOverlay,
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
            egui::Label::new(if checkbox {
                crate::i18n::visible()
            } else {
                crate::i18n::hidden()
            }),
        );
    });

    ui.horizontal(|ui| {
        ui.label(crate::i18n::mask_downsample());
        if ui.button("<").clicked() {
            apply_action(
                SettingsAction::MaskDownsampleDown,
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
                None,
                0.0,
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
    _world: &mut World,
    _ui_entity: Entity,
) {
}
