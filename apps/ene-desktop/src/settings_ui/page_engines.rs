use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;

pub fn render(
    ui: &mut egui::Ui,
    _draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
) {
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "engines-core-hint"
    ));
    input.plugins.poll();
    if !input.plugins.started() {
        input.plugins.start(ai.fetch_plugins());
    }
    section_card(
        ui,
        "engines-plugins",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "engines"),
        |ui| {
            let Some(items) = input.plugins.data.clone() else {
                return;
            };
            for plugin in items {
                ui.label(format!("{} — {}", plugin.plugin, plugin.state));
            }
        },
    );
}
