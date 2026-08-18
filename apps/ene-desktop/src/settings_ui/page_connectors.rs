use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    ui.weak(fl!(crate::i18n::loader(), "connectors-core-hint"));
    input.plugins.poll();
    if !input.plugins.started() {
        input.plugins.start(ai.fetch_plugins());
    }
    section_card(
        ui,
        "connectors-list",
        &fl!(crate::i18n::loader(), "connectors-list"),
        |ui| {
            if ui.button(fl!(crate::i18n::loader(), "refresh")).clicked() {
                input.plugins.restart(ai.fetch_plugins());
            }
            let Some(items) = input.plugins.data.clone() else {
                return;
            };
            for plugin in items {
                ui.label(format!("{} — {}", plugin.plugin, plugin.state));
            }
        },
    );
}
