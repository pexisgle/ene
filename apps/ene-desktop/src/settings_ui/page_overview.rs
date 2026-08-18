use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components;
use super::input::SettingsInputState;
use super::{ApplyFeedback, PageKind};

pub fn render(
    ui: &mut egui::Ui,
    _settings: &CharacterSettings,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    current_page: &mut PageKind,
    feedback: Option<&ApplyFeedback>,
) {
    input.health.poll();
    if !input.health.started() {
        input.health.start(ai.fetch_health());
    }
    input.core_settings.poll();
    if !input.core_settings.started() {
        input.core_settings.start(ai.fetch_core_settings());
    }
    input.plugins.poll();
    if !input.plugins.started() {
        input.plugins.start(ai.fetch_plugins());
    }

    let mut navigate: Option<PageKind> = None;

    components::section_card(
        ui,
        "overview-needs-config",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-needs-config"),
        |ui| {
            match &input.core_settings.data {
                Some(Ok(settings)) => {
                    let plugin = settings
                        .pointer("/effective/ai/tasks/chat/plugin")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("echo");
                    if plugin == "echo" || plugin.is_empty() {
                        ui.weak(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "overview-echo-hint"
                        ));
                    } else {
                        ui.label(plugin);
                    }
                }
                _ => {
                    ui.weak(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "overview-echo-hint"
                    ));
                }
            }
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-models"))
                .clicked()
            {
                navigate = Some(PageKind::Ai);
            }
        },
    );

    components::section_card(
        ui,
        "overview-issues",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-issues"),
        |ui| {
            match &input.health.data {
                Some(Ok(status)) => {
                    ui.label(status);
                }
                Some(Err(err)) => {
                    ui.colored_label(egui::Color32::from_rgb(0xff, 0x8a, 0x65), err);
                }
                None if input.health.loading() => {
                    ui.weak("…");
                }
                None => {
                    ui.weak(ai.bind_label());
                }
            }
            let plugins = input.plugins.data.as_ref().map_or(0, Vec::len);
            ui.label(format!(
                "{}: {plugins}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "tools-and-plugins")
            ));
        },
    );

    components::section_card(
        ui,
        "overview-restart-pending",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-restart-pending"),
        |ui| {
            let pending = feedback.is_some_and(|item| {
                item.ok && (item.impact.plugin_restart || item.impact.app_restart)
            });
            if pending {
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "overview-plugin-restart"
                ));
            } else {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "overview-all-set"
                ));
            }
        },
    );

    if let Some(page) = navigate {
        *current_page = page;
    }
}
