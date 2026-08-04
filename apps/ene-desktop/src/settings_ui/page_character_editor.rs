//! Character card editor page.
//!
//! Allows viewing and editing the local `character.json` (`CCv3` format)
//! in a dedicated settings tab, organized into sections. The page supports
//! validate (with error locations), save (blocked on errors, atomic write
//! with a one-time backup), reload, and a discard-confirmation dialog when
//! the window is closed with unsaved changes.

use super::WARNING_COLOR;
use super::widgets::{SettingsAction, apply_action};
use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::{CharacterSettings, EditorSeverity};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_config::{LorebookEntry, MotionCatalog, MotionEntry, MotionLayer};
use std::path::PathBuf;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    let Some((card_path, _)) = resolve_card_path(settings) else {
        ui.colored_label(
            WARNING_COLOR,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-none-selected"),
        );
        return;
    };

    // ── Auto-load on first render ──
    let needs_load = world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| !s.0.character_editor_loaded);

    if needs_load {
        apply_action(
            SettingsAction::LoadCharacterCard {
                path: card_path.to_string_lossy().to_string(),
            },
            settings,
            &mut crate::character_state::AnimationControl::new(),
            ai,
            world,
            ui_entity,
            None,
            0.0,
        );
    }

    // ── Grab a snapshot of the UiState for rendering ──
    let Some(snapshot) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };

    ui.vertical(|ui| {
        // ── Modified warning ──
        if snapshot.character_editor_modified {
            ui.colored_label(
                egui::Color32::YELLOW,
                i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-modified"),
            );
        }

        // ── Locale-diff notice ──
        if !snapshot.character_editor_locale_diffs.is_empty() {
            ui.colored_label(
                WARNING_COLOR,
                i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-locale-diffs-notice",
                    files = snapshot.character_editor_locale_diffs.join(", ")
                ),
            );
        }

        // ── Validation issues ──
        if !snapshot.character_editor_validation_errors.is_empty() {
            ui.group(|ui| {
                for issue in &snapshot.character_editor_validation_errors {
                    ui.horizontal_wrapped(|ui| {
                        let color = match issue.severity {
                            EditorSeverity::Error => egui::Color32::LIGHT_RED,
                            EditorSeverity::Warning => WARNING_COLOR,
                        };
                        ui.colored_label(color, &issue.location);
                        ui.label(&issue.message);
                    });
                }
            });
            if snapshot
                .character_editor_validation_errors
                .iter()
                .any(|issue| issue.severity == EditorSeverity::Error)
            {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-save-blocked"),
                );
            }
            ui.separator();
        }

        // ── Sections ──
        section(
            ui,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-section-identity"),
            |ui| {
                text_field(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-name"),
                    |s| &mut s.character_editor_name,
                );
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-description"),
                    |s| &mut s.character_editor_description,
                );
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-creator-notes"),
                    |s| &mut s.character_editor_creator_notes,
                );
            },
        );

        section(
            ui,
            i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "character-editor-section-personality"
            ),
            |ui| {
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-personality"),
                    |s| &mut s.character_editor_personality,
                );
            },
        );

        section(
            ui,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-section-scenario"),
            |ui| {
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-scenario"),
                    |s| &mut s.character_editor_scenario,
                );
            },
        );

        section(
            ui,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-section-greetings"),
            |ui| {
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-first-mes"),
                    |s| &mut s.character_editor_first_mes,
                );
                alternate_greetings_editor(ui, world, ui_entity);
            },
        );

        section(
            ui,
            i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "character-editor-section-memory-instructions"
            ),
            |ui| {
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-system-prompt"),
                    |s| &mut s.character_editor_system_prompt,
                );
                text_field(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-post-history"),
                    |s| &mut s.character_editor_post_history,
                );
                text_area(
                    ui,
                    world,
                    ui_entity,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-mes-example"),
                    |s| &mut s.character_editor_mes_example,
                );
            },
        );

        section(
            ui,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-section-lorebook"),
            |ui| lorebook_editor(ui, world, ui_entity),
        );

        section(
            ui,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-section-motions"),
            |ui| motion_editor(ui, world, ui_entity),
        );

        // ── Action buttons ──
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-validate"
                ))
                .clicked()
                && let Some((card_path, assets_dir)) = resolve_card_path(settings)
            {
                apply_action(
                    SettingsAction::ValidateCharacterCard {
                        card_path: card_path.to_string_lossy().to_string(),
                        assets_dir: assets_dir.to_string_lossy().to_string(),
                    },
                    settings,
                    &mut crate::character_state::AnimationControl::new(),
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }

            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-save"
                ))
                .clicked()
                && let Some((card_path, assets_dir)) = resolve_card_path(settings)
            {
                apply_action(
                    SettingsAction::SaveCharacterCard {
                        path: card_path.to_string_lossy().to_string(),
                        assets_dir: assets_dir.to_string_lossy().to_string(),
                    },
                    settings,
                    &mut crate::character_state::AnimationControl::new(),
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }

            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-reload"
                ))
                .clicked()
                && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
            {
                if state.0.editor_has_unsaved_changes() {
                    state.0.character_editor_reload_pending = true;
                } else {
                    state.0.character_editor_loaded = false;
                    state.0.character_editor_validation_errors.clear();
                }
            }
        });
    });
}

/// Discard-confirmation modal shown when the settings window is closed, the
/// app exits, a reload is requested, or a character switch is requested while
/// the editor holds unsaved changes.
pub fn render_discard_dialog(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    emotion_queue: &mut crate::character_state::EmotionQueue,
    now_secs: f64,
    world: &mut World,
    ui_entity: Entity,
) {
    let pending = world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.editor_dialog_pending());
    if !pending {
        return;
    }

    let mut discard = false;
    let mut keep_editing = false;
    egui::Modal::new(egui::Id::new("character-editor-discard-modal")).show(ui.ctx(), |ui| {
        ui.set_min_width(280.0);
        ui.heading(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-discard-title"
        ));
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-discard-body"
        ));
        ui.horizontal(|ui| {
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-discard"
                ))
                .clicked()
            {
                discard = true;
            }
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-keep-editing"
                ))
                .clicked()
            {
                keep_editing = true;
            }
        });
    });

    if !discard && !keep_editing {
        return;
    }
    let mut pending_switch = false;
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        if discard {
            // A deferred character switch still needs settings access, so the
            // world borrow is released before it is applied below.
            pending_switch = super::widgets::apply_discard_decision(&mut state.0);
        } else {
            state.0.cancel_editor_dialog();
        }
    }
    if pending_switch {
        super::widgets::confirm_pending_character_switch(
            settings,
            Some(emotion_queue),
            now_secs,
            world,
            ui_entity,
        );
    }
}

/// Resolve the on-disk paths to the current character's `character.json` and
/// the assets directory.
fn resolve_card_path(settings: &CharacterSettings) -> Option<(PathBuf, PathBuf)> {
    settings
        .current_character_card()
        .map(|rel| (settings.assets_dir.join(rel), settings.assets_dir.clone()))
}

/// A collapsible section; open by default so the whole card is visible.
fn section(ui: &mut egui::Ui, title: String, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(true)
        .show(ui, add);
}

/// Render a single-line text field bound to a `String` field on [`UiState`].
fn text_field(
    ui: &mut egui::Ui,
    world: &mut World,
    ui_entity: Entity,
    label: String,
    accessor: fn(&mut crate::settings::UiState) -> &mut String,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut value = {
            let Some(state) = world.get::<UiStateComponent>(ui_entity) else {
                return;
            };
            accessor(&mut state.0.clone()).clone()
        };
        let response = ui.add(egui::TextEdit::singleline(&mut value).desired_width(300.0));
        if response.changed()
            && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
        {
            *accessor(&mut state.0) = value;
            state.0.character_editor_modified = true;
        }
    });
}

/// Render a multi-line text area bound to a `String` field on [`UiState`].
fn text_area(
    ui: &mut egui::Ui,
    world: &mut World,
    ui_entity: Entity,
    label: String,
    accessor: fn(&mut crate::settings::UiState) -> &mut String,
) {
    ui.horizontal(|ui| {
        ui.label(&label);
        ui.allocate_ui_with_layout(
            egui::vec2(300.0, 80.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let mut value = {
                    let Some(state) = world.get::<UiStateComponent>(ui_entity) else {
                        return;
                    };
                    accessor(&mut state.0.clone()).clone()
                };
                let response = ui.add(
                    egui::TextEdit::multiline(&mut value)
                        .desired_rows(4)
                        .desired_width(300.0),
                );
                if response.changed()
                    && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
                {
                    *accessor(&mut state.0) = value;
                    state.0.character_editor_modified = true;
                }
            },
        );
    });
}

/// Editable list of `data.alternate_greetings`.
fn alternate_greetings_editor(ui: &mut egui::Ui, world: &mut World, ui_entity: Entity) {
    let Some(mut state) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let mut changed = false;
    let mut remove_index = None;

    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "character-editor-alternate-greetings"
    ));
    for (index, greeting) in state
        .character_editor_alternate_greetings
        .iter_mut()
        .enumerate()
    {
        ui.horizontal(|ui| {
            ui.label((index + 1).to_string());
            let response = ui.add(
                egui::TextEdit::multiline(greeting)
                    .desired_rows(2)
                    .desired_width(300.0),
            );
            if response.changed() {
                changed = true;
            }
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-remove"
                ))
                .clicked()
            {
                remove_index = Some(index);
            }
        });
    }

    if ui
        .button(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-add-greeting"
        ))
        .clicked()
    {
        state
            .character_editor_alternate_greetings
            .push(String::new());
        changed = true;
    }
    if let Some(index) = remove_index {
        state.character_editor_alternate_greetings.remove(index);
        changed = true;
    }
    if changed && let Some(mut s) = world.get_mut::<UiStateComponent>(ui_entity) {
        s.0.character_editor_alternate_greetings = state.character_editor_alternate_greetings;
        s.0.character_editor_modified = true;
    }
}

/// Structured row editor for `data.character_book.entries`.
fn lorebook_editor(ui: &mut egui::Ui, world: &mut World, ui_entity: Entity) {
    let Some(mut state) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let mut changed = false;
    let mut remove_index = None;

    let Some(book) = &mut state.character_editor_lorebook else {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-lorebook-no-entries"
        ));
        if ui
            .button(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "character-editor-lorebook-add-entry"
            ))
            .clicked()
        {
            state.character_editor_lorebook = Some(ene_config::Lorebook {
                entries: vec![new_lorebook_entry(0)],
                ..ene_config::Lorebook::default()
            });
            changed = true;
        }
        if changed && let Some(mut s) = world.get_mut::<UiStateComponent>(ui_entity) {
            s.0.character_editor_lorebook = state.character_editor_lorebook;
            s.0.character_editor_modified = true;
        }
        return;
    };

    for (index, entry) in book.entries.iter_mut().enumerate() {
        let header = entry
            .name
            .clone()
            .unwrap_or_else(|| format!("Entry {}", index + 1));
        let remove_label = i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-remove");
        egui::CollapsingHeader::new(header)
            .id_salt(("lorebook-entry", index))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut enabled = entry.enabled;
                    if ui
                        .checkbox(
                            &mut enabled,
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-enabled"
                            ),
                        )
                        .changed()
                    {
                        entry.enabled = enabled;
                        changed = true;
                    }
                    let mut constant = entry.constant.unwrap_or(false);
                    if ui
                        .checkbox(
                            &mut constant,
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-constant"
                            ),
                        )
                        .changed()
                    {
                        entry.constant = Some(constant);
                        changed = true;
                    }
                });
                let mut name = entry.name.clone().unwrap_or_default();
                let name_changed = singleline_field(
                    ui,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-lorebook-name"),
                    &mut name,
                );
                if name_changed {
                    // An emptied name goes back to `null` rather than `""` so
                    // a no-edit save stays byte-identical.
                    entry.name = if name.is_empty() { None } else { Some(name) };
                    changed = true;
                }
                changed |= key_list_field(
                    ui,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-lorebook-keys"),
                    &mut entry.keys,
                );
                let mut secondary = entry.secondary_keys.clone().unwrap_or_default();
                let secondary_changed = key_list_field(
                    ui,
                    i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-lorebook-secondary-keys"
                    ),
                    &mut secondary,
                );
                if secondary_changed {
                    entry.secondary_keys = if secondary.is_empty() {
                        None
                    } else {
                        Some(secondary)
                    };
                    changed = true;
                }
                changed |= multiline_field(
                    ui,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-lorebook-content"),
                    &mut entry.content,
                );
                ui.horizontal(|ui| {
                    let mut use_regex = entry.use_regex;
                    if ui
                        .checkbox(
                            &mut use_regex,
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-use-regex"
                            ),
                        )
                        .changed()
                    {
                        entry.use_regex = use_regex;
                        changed = true;
                    }
                    let mut case_sensitive = entry.case_sensitive.unwrap_or(false);
                    if ui
                        .checkbox(
                            &mut case_sensitive,
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-case-sensitive"
                            ),
                        )
                        .changed()
                    {
                        entry.case_sensitive = Some(case_sensitive);
                        changed = true;
                    }
                    let mut selective = entry.selective.unwrap_or(false);
                    if ui
                        .checkbox(
                            &mut selective,
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-selective"
                            ),
                        )
                        .changed()
                    {
                        entry.selective = Some(selective);
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-lorebook-position"
                    ));
                    let mut position = entry.position.clone().unwrap_or_default();
                    egui::ComboBox::from_id_salt(("lorebook-position", index))
                        .selected_text(if position.is_empty() {
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-position-default"
                            )
                        } else {
                            position.clone()
                        })
                        .show_ui(ui, |ui| {
                            for value in ["", "before_char", "after_char"] {
                                let label = if value.is_empty() {
                                    i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "character-editor-position-default"
                                    )
                                } else {
                                    value.to_string()
                                };
                                if ui.selectable_label(position == value, label).clicked() {
                                    position = value.to_string();
                                    changed = true;
                                }
                            }
                        });
                    entry.position = if position.is_empty() {
                        None
                    } else {
                        Some(position)
                    };
                    let mut priority = entry.priority.unwrap_or(0);
                    if ui
                        .add(egui::DragValue::new(&mut priority).prefix(format!(
                            "{} ",
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-priority"
                            )
                        )))
                        .changed()
                    {
                        entry.priority = Some(priority);
                        changed = true;
                    }
                    let mut insertion_order = entry.insertion_order;
                    if ui
                        .add(egui::DragValue::new(&mut insertion_order).prefix(format!(
                            "{} ",
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "character-editor-lorebook-order"
                            )
                        )))
                        .changed()
                    {
                        entry.insertion_order = insertion_order;
                        changed = true;
                    }
                });
                let mut comment = entry.comment.clone().unwrap_or_default();
                let comment_changed = singleline_field(
                    ui,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-lorebook-comment"),
                    &mut comment,
                );
                if comment_changed {
                    entry.comment = if comment.is_empty() {
                        None
                    } else {
                        Some(comment)
                    };
                    changed = true;
                }
                if ui.button(remove_label).clicked() {
                    remove_index = Some(index);
                }
            });
    }

    if ui
        .button(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-lorebook-add-entry"
        ))
        .clicked()
    {
        book.entries.push(new_lorebook_entry(book.entries.len()));
        changed = true;
    }
    if let Some(index) = remove_index {
        book.entries.remove(index);
        changed = true;
    }
    if changed && let Some(mut s) = world.get_mut::<UiStateComponent>(ui_entity) {
        s.0.character_editor_lorebook = state.character_editor_lorebook;
        s.0.character_editor_modified = true;
    }
}

/// Row editor for `extensions.ene.motion_catalog`.
fn motion_editor(ui: &mut egui::Ui, world: &mut World, ui_entity: Entity) {
    let Some(mut state) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let mut changed = false;
    let mut remove_index = None;

    let Some(catalog) = &mut state.character_editor_motion_catalog else {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-motion-no-motions"
        ));
        if ui
            .button(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "character-editor-motion-add"
            ))
            .clicked()
        {
            state.character_editor_motion_catalog = Some(MotionCatalog {
                motions: vec![new_motion_entry()],
                ..MotionCatalog::default()
            });
            changed = true;
        }
        if changed && let Some(mut s) = world.get_mut::<UiStateComponent>(ui_entity) {
            s.0.character_editor_motion_catalog = state.character_editor_motion_catalog;
            s.0.character_editor_modified = true;
        }
        return;
    };

    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-motion-idle-lower"
        ));
        let names = catalog
            .motions
            .iter()
            .map(|motion| motion.name.clone())
            .collect::<Vec<_>>();
        let mut idle = catalog.idle_lower.clone();
        egui::ComboBox::from_id_salt("motion-idle-lower")
            .selected_text(idle.clone().unwrap_or_else(|| {
                i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-motion-idle-none")
            }))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        idle.is_none(),
                        i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "character-editor-motion-idle-none"
                        ),
                    )
                    .clicked()
                {
                    idle = None;
                    changed = true;
                }
                for name in names {
                    if ui
                        .selectable_label(idle.as_deref() == Some(name.as_str()), &name)
                        .clicked()
                    {
                        idle = Some(name);
                        changed = true;
                    }
                }
            });
        if catalog.idle_lower != idle {
            catalog.idle_lower = idle;
            changed = true;
        }
    });

    for (index, motion) in catalog.motions.iter_mut().enumerate() {
        let header = if motion.name.is_empty() {
            format!("Motion {}", index + 1)
        } else {
            motion.name.clone()
        };
        egui::CollapsingHeader::new(header)
            .id_salt(("motion-entry", index))
            .show(ui, |ui| {
                changed |= singleline_field(
                    ui,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-motion-name"),
                    &mut motion.name,
                );
                changed |= singleline_field(
                    ui,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-motion-path"),
                    &mut motion.path,
                );
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-motion-layer"
                    ));
                    let mut layer = motion.layer;
                    egui::ComboBox::from_id_salt(("motion-layer", index))
                        .selected_text(layer_label(layer))
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    layer.is_none(),
                                    i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "character-editor-motion-layer-default"
                                    ),
                                )
                                .clicked()
                            {
                                layer = None;
                                changed = true;
                            }
                            for candidate in
                                [MotionLayer::Upper, MotionLayer::Lower, MotionLayer::Full]
                            {
                                if ui
                                    .selectable_label(
                                        layer == Some(candidate),
                                        layer_label(Some(candidate)),
                                    )
                                    .clicked()
                                {
                                    layer = Some(candidate);
                                    changed = true;
                                }
                            }
                        });
                    motion.layer = layer;
                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "character-editor-remove"
                        ))
                        .clicked()
                    {
                        remove_index = Some(index);
                    }
                });
            });
    }

    if ui
        .button(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-motion-add"
        ))
        .clicked()
    {
        catalog.motions.push(new_motion_entry());
        changed = true;
    }
    if let Some(index) = remove_index {
        catalog.motions.remove(index);
        changed = true;
    }
    if changed && let Some(mut s) = world.get_mut::<UiStateComponent>(ui_entity) {
        s.0.character_editor_motion_catalog = state.character_editor_motion_catalog;
        s.0.character_editor_modified = true;
    }
}

/// A new motion row with empty fields; the validation pass flags a missing
/// name/path until the user fills them in.
fn new_motion_entry() -> MotionEntry {
    MotionEntry {
        name: String::new(),
        path: String::new(),
        layer: None,
        extra: indexmap::IndexMap::new(),
    }
}

/// A new lorebook entry with sensible defaults; `insertion_order` places it
/// after the entries already present.
fn new_lorebook_entry(insertion_order: usize) -> LorebookEntry {
    LorebookEntry {
        keys: Vec::new(),
        content: String::new(),
        extensions: std::collections::HashMap::new(),
        enabled: true,
        insertion_order: insertion_order as i32,
        case_sensitive: None,
        use_regex: false,
        constant: Some(false),
        name: None,
        priority: None,
        id: None,
        comment: None,
        selective: None,
        secondary_keys: None,
        position: None,
        extra: indexmap::IndexMap::new(),
    }
}

/// A single-line field bound to a string owned by the caller; returns whether
/// the value changed so the caller can mark the card modified.
fn singleline_field(ui: &mut egui::Ui, label: String, value: &mut String) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(300.0))
            .changed()
    })
    .inner
}

/// A multi-line field bound to a string owned by the caller.
fn multiline_field(ui: &mut egui::Ui, label: String, value: &mut String) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::TextEdit::multiline(value)
                .desired_rows(3)
                .desired_width(300.0),
        )
        .changed()
    })
    .inner
}

/// Newline-separated list field (trigger keys, secondary keys). Blank lines
/// are dropped on write.
fn key_list_field(ui: &mut egui::Ui, label: String, keys: &mut Vec<String>) -> bool {
    let joined = keys.join("\n");
    let mut edited = joined;
    let changed = ui
        .horizontal(|ui| {
            ui.label(label);
            ui.add(
                egui::TextEdit::multiline(&mut edited)
                    .desired_rows(3)
                    .desired_width(300.0),
            )
            .changed()
        })
        .inner;
    if changed {
        keys.clear();
        keys.extend(
            edited
                .split('\n')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string),
        );
    }
    changed
}

/// Localized label for a motion layer (or the default when unset).
fn layer_label(layer: Option<MotionLayer>) -> String {
    match layer {
        Some(MotionLayer::Upper) => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-motion-layer-upper")
        }
        Some(MotionLayer::Lower) => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-motion-layer-lower")
        }
        Some(MotionLayer::Full) => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-motion-layer-full")
        }
        None => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-motion-layer-default"
        ),
    }
}
