use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use crate::component::ui::UiStateComponent;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

pub fn render_config(ui: &mut egui::Ui, _settings: &CharacterSettings, draft: &mut SettingsDraft) {
    section_card(
        ui,
        "memory-config",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memory-storage"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "memory-core-hint"
            ));
            let mut text = draft
                .editing()
                .section_value("mind")
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| "{}".to_owned());
            if ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(10),
                )
                .changed()
                && let Ok(parsed) = serde_json::from_str(&text)
            {
                draft.set_section_value("mind", parsed);
            }
        },
    );
}

pub fn render_journal(
    ui: &mut egui::Ui,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    input.memories.poll();
    if !input.memories.started() {
        input.memories.start(ai.fetch_memories());
    }
    let pending = world
        .get::<UiStateComponent>(ui_entity)
        .map_or(0, |state| state.0.memory_journal_pending_count);
    section_card(
        ui,
        "memory-journal",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memories"),
        |ui| {
            if pending > 0 {
                ui.label(format!(
                    "{}: {pending}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "memory-page-tab-pending")
                ));
            }
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "refresh"))
                .clicked()
            {
                input.memories.restart(ai.fetch_memories());
            }
            if input.memories.loading() {
                ui.weak("…");
            }
            if let Some(err) = &input.memories.error {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x8a, 0x65), err);
            }
            let Some(items) = input.memories.data.clone() else {
                return;
            };
            for item in items {
                ui.separator();
                ui.strong(&item.title);
                ui.label(format!("{} · {}", item.kind, item.scope));
                ui.label(&item.content);
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "delete"))
                    .clicked()
                {
                    drop(ai.delete_memory(item.id.clone()));
                    input.memories.restart(ai.fetch_memories());
                }
            }
        },
    );
}
