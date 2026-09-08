use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    page_memory_like(ui, ai, input);
}

fn page_memory_like(ui: &mut egui::Ui, ai: &Arc<CoreSession>, input: &mut SettingsInputState) {
    input.memories.poll();
    if !input.memories.started() {
        input.memories.start(ai.fetch_memories());
    }
    section_card(
        ui,
        "memory-ledger",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memories"),
        |ui| {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "refresh"))
                .clicked()
            {
                input.memories.restart(ai.fetch_memories());
            }
            let Some(items) = input.memories.data.clone() else {
                return;
            };
            for item in items {
                ui.separator();
                ui.label(format!("{} [{}]", item.title, item.id));
                let mut content = item.content.clone();
                if ui.text_edit_multiline(&mut content).changed() {
                    drop(ai.patch_memory(item.id.clone(), content));
                }
            }
        },
    );
}
