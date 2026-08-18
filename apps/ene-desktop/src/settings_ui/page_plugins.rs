use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};

const PROFILES: &[&str] = &["desktop", "minimal", "headless"];

pub fn render(
    ui: &mut egui::Ui,
    _settings: &crate::settings::CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
    plugin_focus: Option<&str>,
) {
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "page-tools-and-plugins-description"
    ));
    input.core_settings.poll();
    if !input.core_settings.started() {
        input.core_settings.start(ai.fetch_core_settings());
    }
    if let Some(Ok(settings)) = input.core_settings.data.clone() {
        seed_draft_once(draft, &settings);
    }

    section_card(
        ui,
        "plugins-profile",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-profile-hint"
            ));
            let mut plugins = current_plugins(draft);
            let selected = plugins
                .get("profile")
                .and_then(Value::as_str)
                .unwrap_or("desktop")
                .to_owned();
            let mut next = selected.clone();
            egui::ComboBox::from_id_salt("plugins-profile")
                .selected_text(profile_label(&selected))
                .show_ui(ui, |ui| {
                    for id in PROFILES {
                        ui.selectable_value(&mut next, (*id).to_owned(), profile_label(id));
                    }
                });
            ui.weak(profile_hint(&next));
            if next != selected {
                plugins["profile"] = json!(next);
                draft.set_section_value("plugins", plugins);
            }
        },
    );

    input.plugins.poll();
    if !input.plugins.started() {
        input.plugins.start(ai.fetch_plugins());
    }
    section_card(
        ui,
        "plugins-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-running"),
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
            if items.is_empty() {
                ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-none"));
                return;
            }
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
                if let Some(reason) = &plugin.wait_reason {
                    ui.weak(reason);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-restart"))
                    .clicked()
                {
                    drop(ai.restart_plugin(plugin.row_id.clone()));
                    input.plugins.restart(ai.fetch_plugins());
                }
            }
        },
    );
}

fn seed_draft_once(draft: &mut SettingsDraft, settings: &Value) {
    if draft.editing().section_value("plugins").is_some() {
        return;
    }
    if let Some(plugins) = settings.pointer("/effective/plugins") {
        draft.seed_core_section("plugins", plugins.clone());
    }
}

fn current_plugins(draft: &SettingsDraft) -> Value {
    draft
        .editing()
        .section_value("plugins")
        .unwrap_or_else(|| json!({"profile": "desktop"}))
}

fn profile_label(id: &str) -> String {
    match id {
        "minimal" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile-minimal"),
        "headless" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile-headless"),
        _ => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile-desktop"),
    }
}

fn profile_hint(id: &str) -> String {
    match id {
        "minimal" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile-minimal-hint"),
        "headless" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile-headless-hint"),
        _ => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-profile-desktop-hint"),
    }
}
