use std::sync::Arc;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_api::McpDocument;
use i18n_embed_fl::fl;

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    ui.weak(fl!(crate::i18n::loader(), "connectors-core-hint"));
    input.mcp.poll();
    if !input.mcp.started() {
        input.mcp.start(ai.fetch_mcp());
    }
    if let Some(Ok(doc)) = input.mcp.data.as_ref()
        && input.mcp_json.is_empty()
        && let Ok(text) = serde_json::to_string_pretty(doc)
    {
        input.mcp_json = text;
    }
    input.plugins.poll();
    if !input.plugins.started() {
        input.plugins.start(ai.fetch_plugins());
    }
    section_card(
        ui,
        "connectors-list",
        &fl!(crate::i18n::loader(), "connectors-list"),
        |ui| {
            ui.label(fl!(crate::i18n::loader(), "connectors-mcp-hint"));
            let editor = egui::TextEdit::multiline(&mut input.mcp_json)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(16);
            ui.add(editor);
            ui.horizontal(|ui| {
                if ui
                    .button(fl!(crate::i18n::loader(), "connectors-mcp-save"))
                    .clicked()
                {
                    match serde_json::from_str::<McpDocument>(&input.mcp_json) {
                        Ok(doc) => {
                            input.mcp_status = None;
                            input.mcp.restart(ai.put_mcp(doc));
                        }
                        Err(err) => {
                            input.mcp_status = Some(format!(
                                "{}: {err}",
                                fl!(crate::i18n::loader(), "connectors-mcp-invalid")
                            ));
                        }
                    }
                }
                if ui
                    .button(fl!(crate::i18n::loader(), "connectors-mcp-reload"))
                    .clicked()
                {
                    input.mcp_json.clear();
                    input.mcp.restart(ai.fetch_mcp());
                    input.plugins.restart(ai.fetch_plugins());
                }
            });
            if let Some(Err(err)) = input.mcp.data.as_ref() {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            if let Some(status) = &input.mcp_status {
                ui.colored_label(ui.visuals().error_fg_color, status);
            }
        },
    );
    section_card(
        ui,
        "connectors-detail",
        &fl!(crate::i18n::loader(), "connectors-status-title"),
        |ui| {
            ui.label(fl!(crate::i18n::loader(), "connectors-mcp-detail"));
            let Some(items) = input.plugins.data.clone() else {
                return;
            };
            let mcp_rows: Vec<_> = items
                .into_iter()
                .filter(|plugin| plugin.plugin.starts_with("mcp."))
                .collect();
            if mcp_rows.is_empty() {
                ui.weak(fl!(crate::i18n::loader(), "connectors-fibers-empty"));
                return;
            }
            for plugin in mcp_rows {
                ui.label(format!("{} — {}", plugin.plugin, plugin.state));
            }
        },
    );
}
