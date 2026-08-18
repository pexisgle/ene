use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

pub fn render(
    ui: &mut egui::Ui,
    _settings: &crate::settings::CharacterSettings,
    _draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
    plugin_focus: Option<&str>,
) {
    input.plugins.poll();
    if !input.plugins.started() {
        input.plugins.start(ai.fetch_plugins());
    }
    section_card(
        ui,
        "plugins-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "tools-and-plugins"),
        |ui| {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "refresh"))
                .clicked()
            {
                input.plugins.restart(ai.fetch_plugins());
            }
            let Some(items) = input.plugins.data.clone() else {
                return;
            };
            for plugin in items {
                let focused = plugin_focus == Some(plugin.plugin.as_str());
                ui.separator();
                let heading = if focused {
                    format!("▸ {} ({})", plugin.plugin, plugin.state)
                } else {
                    format!("{} ({})", plugin.plugin, plugin.state)
                };
                ui.strong(heading);
                ui.label(&plugin.row_id);
                if ui.button("restart").clicked() {
                    drop(ai.restart_plugin(plugin.row_id.clone()));
                    input.plugins.restart(ai.fetch_plugins());
                }
            }
        },
    );
}
