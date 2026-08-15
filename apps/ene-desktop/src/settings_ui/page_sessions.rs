//! All actor round-trips run on the bridge runtime through [`AsyncData`]
//! receivers; the render thread only polls receivers and renders results.
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::SessionSearchRow;
use crate::settings_ui::components::{
    BadgeTone, empty_state, section_card, setting_row, status_badge, toggle_row,
};
use crate::settings_ui::input::{AsyncData, SettingsInputState};

const LIST_LIMIT: usize = 50;
const SEARCH_LIMIT: usize = 20;
const CONTENT_PREVIEW_LEN: usize = 120;

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    input.sessions.poll();
    if !input.sessions.started() {
        refresh_sessions(ai, input, world, ui_entity);
    }
    input.session_search.poll();
    input.session_message.poll();

    ui.vertical(|ui| {
        section_card(
            ui,
            "sessions-search",
            &fl!(crate::i18n::loader(), "sessions-search-title"),
            |ui| render_search(ui, ai, input, world, ui_entity),
        );
        section_card(
            ui,
            "sessions-import",
            &fl!(crate::i18n::loader(), "sessions-import-title"),
            |ui| render_import(ui, ai, input, world, ui_entity),
        );
        section_card(
            ui,
            "sessions-list",
            &fl!(crate::i18n::loader(), "sessions-list-title"),
            |ui| render_list(ui, ai, input, world, ui_entity),
        );
    });

    if let Some(message) = input.session_message.data.clone() {
        ui.add_space(6.0);
        ui.label(message);
    }
}

fn render_search(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let mut do_search = false;
    setting_row(
        ui,
        "session_search_row",
        &fl!(crate::i18n::loader(), "sessions-hint"),
        "",
        |ui| {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui.add(
                    egui::TextEdit::singleline(&mut state.0.session_search_query)
                        .desired_width(240.0)
                        .hint_text(fl!(crate::i18n::loader(), "sessions-search-placeholder")),
                );
            }
            if ui
                .button(fl!(crate::i18n::loader(), "sessions-search"))
                .clicked()
            {
                do_search = true;
            }
        },
    );

    if do_search {
        run_search(ai, input, world, ui_entity);
    }
    if input.session_search.loading() {
        ui.weak(fl!(crate::i18n::loader(), "sessions-loading"));
        return;
    }

    let rows: Vec<SessionSearchRow> = input
        .session_search
        .data
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(session_id, message)| SessionSearchRow {
            session_id,
            role: message.role,
            content: truncate_preview(&message.content),
        })
        .collect();
    if rows.is_empty() {
        empty_state(ui, &fl!(crate::i18n::loader(), "sessions-search-empty"), "");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("session_search_scroll")
        .max_height(200.0)
        .show(ui, |ui| {
            for row in &rows {
                render_search_row(ui, row);
                ui.add_space(4.0);
            }
        });
}

fn render_search_row(ui: &mut egui::Ui, row: &SessionSearchRow) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(format!(
                "{}: {}  |  {}: {}",
                fl!(crate::i18n::loader(), "sessions-col-session"),
                row.session_id,
                fl!(crate::i18n::loader(), "sessions-col-role"),
                row.role,
            ));
            ui.add(egui::Label::new(row.content.as_str()).wrap());
        });
}

fn render_import(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let mut do_import = false;
    setting_row(
        ui,
        "session_import_row",
        &fl!(crate::i18n::loader(), "sessions-hint"),
        "",
        |ui| {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                super::widgets::path_row(ui, &mut state.0.session_import_path, 240.0);
            }
            if ui
                .button(fl!(crate::i18n::loader(), "sessions-import"))
                .clicked()
            {
                do_import = true;
            }
        },
    );

    if do_import {
        run_import(ai, input, world, ui_entity);
    }
}

fn render_list(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    setting_row(
        ui,
        "session_list_refresh_row",
        &fl!(crate::i18n::loader(), "sessions-hint"),
        "",
        |ui| {
            if ui
                .button(fl!(crate::i18n::loader(), "sessions-refresh"))
                .clicked()
            {
                refresh_sessions(ai, input, world, ui_entity);
            }
        },
    );

    let mut show_archived = world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.session_show_archived);
    if toggle_row(
        ui,
        "session_show_archived_row",
        &fl!(crate::i18n::loader(), "sessions-show-archived"),
        "",
        &mut show_archived,
    ) {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            state.0.session_show_archived = show_archived;
        }
        refresh_sessions(ai, input, world, ui_entity);
    }

    if input.sessions.loading() {
        ui.weak(fl!(crate::i18n::loader(), "sessions-loading"));
        return;
    }
    if let Some(error) = input.sessions.error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
        return;
    }

    let sessions = input.sessions.data.clone().unwrap_or_default();
    if sessions.is_empty() {
        empty_state(ui, &fl!(crate::i18n::loader(), "sessions-empty"), "");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("session_list_scroll")
        .max_height(300.0)
        .show(ui, |ui| {
            for session in &sessions {
                render_session_row(ui, ai, input, world, ui_entity, session);
                ui.add_space(4.0);
            }
        });
}

fn render_session_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    session: &ene_runtime::PublicSessionMeta,
) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            let title = if session.title.is_empty() {
                fl!(crate::i18n::loader(), "sessions-untitled")
            } else {
                session.title.clone()
            };
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{}: {}  |  {}: {}",
                    fl!(crate::i18n::loader(), "sessions-col-session"),
                    session.session_id,
                    fl!(crate::i18n::loader(), "sessions-col-title"),
                    title,
                ));
                if session.archived {
                    status_badge(
                        ui,
                        &fl!(crate::i18n::loader(), "sessions-archived-badge"),
                        BadgeTone::Neutral,
                    );
                }
            });
            ui.label(format!(
                "{}: {}  |  {}: {}",
                fl!(crate::i18n::loader(), "sessions-col-turns"),
                session.turn_count,
                fl!(crate::i18n::loader(), "sessions-col-updated"),
                session.updated_at.format("%Y-%m-%d %H:%M:%S UTC"),
            ));
            ui.horizontal(|ui| {
                if session.archived {
                    if ui
                        .button(fl!(crate::i18n::loader(), "sessions-unarchive"))
                        .clicked()
                    {
                        set_archived(ai, input, world, ui_entity, &session.session_id, false);
                    }
                } else if ui
                    .button(fl!(crate::i18n::loader(), "sessions-archive"))
                    .clicked()
                {
                    set_archived(ai, input, world, ui_entity, &session.session_id, true);
                }
                if ui
                    .button(fl!(crate::i18n::loader(), "sessions-export"))
                    .clicked()
                {
                    export_session(ai, input, &session.session_id);
                }
            });
        });
}

fn refresh_sessions(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let include_archived = world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.session_show_archived);
    input.sessions = AsyncData::new();
    input
        .sessions
        .start(ai.fetch_sessions(include_archived, LIST_LIMIT));
}

fn run_search(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let query = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.session_search_query.clone())
        .unwrap_or_default();
    if query.trim().is_empty() {
        set_message(
            input,
            fl!(crate::i18n::loader(), "sessions-search-query-required"),
        );
        return;
    }
    input.session_search = AsyncData::new();
    input
        .session_search
        .start(ai.fetch_session_search(query, SEARCH_LIMIT));
}

fn run_import(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let path = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.session_import_path.clone())
        .unwrap_or_default();
    if path.trim().is_empty() {
        set_message(
            input,
            fl!(crate::i18n::loader(), "sessions-import-path-required"),
        );
        return;
    }
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) => {
            set_message(
                input,
                format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "sessions-import-read-error")
                ),
            );
            return;
        }
    };
    input.session_message = AsyncData::new();
    let ok_label = fl!(crate::i18n::loader(), "sessions-import-ok");
    let error_label = fl!(crate::i18n::loader(), "sessions-import-error");
    let receiver = ai.apply_import_session(json);
    let message = ai.spawn_fetch(async move {
        match receiver.await {
            Ok(Ok(_)) => ok_label,
            Ok(Err(error)) => format!("{error_label}: {error}"),
            Err(_) => error_label,
        }
    });
    input.session_message.start(message);
    refresh_sessions(ai, input, world, ui_entity);
}

fn set_archived(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    session_id: &str,
    archived: bool,
) {
    let (ok_key, error_key) = if archived {
        (
            fl!(crate::i18n::loader(), "sessions-archive-ok"),
            fl!(crate::i18n::loader(), "sessions-archive-error"),
        )
    } else {
        (
            fl!(crate::i18n::loader(), "sessions-unarchive-ok"),
            fl!(crate::i18n::loader(), "sessions-unarchive-error"),
        )
    };
    input.session_message = AsyncData::new();
    let receiver = ai.apply_archive_session(session_id.to_string(), archived);
    let message = ai.spawn_fetch(async move {
        match receiver.await {
            Ok(Ok(_)) => ok_key,
            Ok(Err(error)) => format!("{error_key}: {error}"),
            Err(_) => error_key,
        }
    });
    input.session_message.start(message);
    refresh_sessions(ai, input, world, ui_entity);
}

fn export_session(ai: &Arc<AiBridge>, input: &mut SettingsInputState, session_id: &str) {
    let owned_session_id = session_id.to_string();
    let receiver = ai.apply_export_session(owned_session_id.clone());
    let ok_label = fl!(crate::i18n::loader(), "sessions-export-ok");
    let error_label = fl!(crate::i18n::loader(), "sessions-export-error");
    input.session_message = AsyncData::new();
    let message = ai.spawn_fetch(async move {
        match receiver.await {
            Ok(Ok(json)) => match write_export(&owned_session_id, &json) {
                Ok(path) => format!("{ok_label}: {}", path.display()),
                Err(error) => format!("{error_label}: {error}"),
            },
            Ok(Err(error)) => format!("{error_label}: {error}"),
            Err(_) => error_label,
        }
    });
    input.session_message.start(message);
}

/// Write an exported session to `<app data>/exports/<session_id>.json`,
/// creating the directory as needed. The desktop app has no native file
/// dialog, so a deterministic path under the app data directory is used
/// and surfaced in the status line.
fn write_export(session_id: &str, json: &str) -> Result<std::path::PathBuf, String> {
    let dir = ene_config::app_data_dir().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", sanitize_file_name(session_id)));
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Replace path-hostile characters so a session id is safe to use as a
/// file name.
fn sanitize_file_name(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect()
}

fn truncate_preview(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= CONTENT_PREVIEW_LEN {
        return trimmed.to_string();
    }
    let mut preview: String = trimmed.chars().take(CONTENT_PREVIEW_LEN).collect();
    preview.push('…');
    preview
}

fn set_message(input: &mut SettingsInputState, message: String) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if tx.send(message).is_err() {
        tracing::debug!(component = "SessionsPage", "message dropped");
    }
    input.session_message = AsyncData::new();
    input.session_message.start(rx);
}
