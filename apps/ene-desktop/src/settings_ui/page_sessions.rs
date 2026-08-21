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
    input.sessions.poll();
    if !input.sessions.started() {
        input.sessions.start(ai.fetch_sessions());
    }
    section_card(
        ui,
        "sessions-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "sessions"),
        |ui| {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "refresh"))
                .clicked()
            {
                input.sessions.restart(ai.fetch_sessions());
            }
            let Some(items) = input.sessions.data.clone() else {
                return;
            };
            for session in items {
                ui.separator();
                ui.strong(session.title.clone().unwrap_or_else(|| session.id.clone()));
                ui.label(format!(
                    "{} · {} · archived={}",
                    session.kind, session.created_at, session.archived
                ));
                ui.horizontal(|ui| {
                    if ui.button("archive").clicked() {
                        drop(ai.archive_session(session.id.clone(), !session.archived));
                        input.sessions.restart(ai.fetch_sessions());
                    }
                    if ui.button("fork").clicked() {
                        drop(ai.fork_session(session.id.clone()));
                        input.sessions.restart(ai.fetch_sessions());
                    }
                    if ui.button("export").clicked() {
                        input
                            .session_export
                            .restart(ai.export_session(session.id.clone()));
                    }
                });
            }
            input.session_export.poll();
            if let Some(Ok(value)) = &input.session_export.data {
                ui.collapsing("export", |ui| {
                    ui.monospace(value.to_string());
                });
            }
        },
    );
}
