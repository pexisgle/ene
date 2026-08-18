use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use super::provider_form::{
    BUILTIN_PROVIDER_I18N_IDS, provider_description, provider_display_group, provider_display_name,
};

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
        "engines-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "engines-list"),
        |ui| {
            for (kind, _) in BUILTIN_PROVIDER_I18N_IDS {
                let plugin = if *kind == "echo" {
                    (*kind).to_owned()
                } else {
                    format!("provider.{kind}")
                };
                ui.separator();
                ui.strong(provider_display_name(&plugin));
                if let Some(group) = provider_display_group(&plugin) {
                    ui.weak(group);
                }
                if let Some(desc) = provider_description(&plugin) {
                    ui.label(desc);
                }
            }
        },
    );
    section_card(
        ui,
        "engines-plugins",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "engines"),
        |ui| {
            let Some(items) = input.plugins.data.clone() else {
                return;
            };
            if items.is_empty() {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "engines-live-empty"
                ));
                return;
            }
            for plugin in items {
                ui.label(format!("{} — {}", plugin.plugin, plugin.state));
            }
        },
    );
}
