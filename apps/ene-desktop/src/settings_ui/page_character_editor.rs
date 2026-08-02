//! Character card editor page.
//!
//! Allows viewing and editing the local `character.json` (`CCv3` format)
//! in a dedicated settings tab. Supports validate, save, and reload.

use super::WARNING_COLOR;
use super::widgets::{SettingsAction, apply_action};
use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::path::PathBuf;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    let Some(card_rel) = settings.current_character_card() else {
        ui.colored_label(
            WARNING_COLOR,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-asset-none-selected"),
        );
        return;
    };
    let card_path = settings.assets_dir.join(card_rel);

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
            ui.separator();
        }

        // ── Validation errors ──
        if !snapshot.character_editor_validation_errors.is_empty() {
            ui.group(|ui| {
                for err in &snapshot.character_editor_validation_errors {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
            });
            ui.separator();
        }

        // ── Text fields (bound to live UiState) ──
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
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-personality"),
            |s| &mut s.character_editor_personality,
        );

        text_area(
            ui,
            world,
            ui_entity,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-scenario"),
            |s| &mut s.character_editor_scenario,
        );

        text_area(
            ui,
            world,
            ui_entity,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-system-prompt"),
            |s| &mut s.character_editor_system_prompt,
        );

        text_area(
            ui,
            world,
            ui_entity,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-mes-example"),
            |s| &mut s.character_editor_mes_example,
        );

        text_area(
            ui,
            world,
            ui_entity,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-first-mes"),
            |s| &mut s.character_editor_first_mes,
        );

        text_field(
            ui,
            world,
            ui_entity,
            i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-post-history"),
            |s| &mut s.character_editor_post_history,
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
            {
                apply_action(
                    SettingsAction::ValidateCharacterCard,
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
                && let Some(path) = resolve_card_path(settings)
            {
                apply_action(
                    SettingsAction::SaveCharacterCard {
                        path: path.to_string_lossy().to_string(),
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
            {
                // Reset the loaded flag so the auto-load path re-reads the file.
                if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                    state.0.character_editor_loaded = false;
                    state.0.character_editor_validation_errors.clear();
                }
            }
        });
    });
}

/// Resolve the full on-disk path to the current character's `character.json`.
fn resolve_card_path(settings: &CharacterSettings) -> Option<PathBuf> {
    settings
        .current_character_card()
        .map(|rel| settings.assets_dir.join(rel))
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
