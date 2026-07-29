//! Sessions settings page (#176).
//!
//! Surfaces the stored-session lifecycle over the actor's session API:
//!
//! * **List** — session metadata (id, title, turn count, last update)
//!   fetched via [`AiBridge::list_sessions_blocking`], lazy-loaded on
//!   first view and refreshed after every archive / import action.
//! * **Search** — free-text message search via
//!   [`AiBridge::search_sessions_blocking`], showing matching messages
//!   with their session id, role, and truncated content.
//! * **Archive / Unarchive** — per-row toggles via
//!   [`AiBridge::archive_session_blocking`].
//! * **Export** — per-row JSON export via
//!   [`AiBridge::export_session_blocking`], written to the app data
//!   directory (the desktop app has no native file dialog).
//! * **Import** — reads a JSON file from a typed path and feeds it to
//!   [`AiBridge::import_session_blocking`].
//!
//! The page mirrors the data-fetch pattern of [`super::page_permissions`]:
//! blocking calls run on the UI thread through the bridge's tokio
//! handle, results are cached on [`UiStateComponent`], and a status
//! line reports the outcome of the last operation.
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::SessionSearchRow;

/// Maximum number of sessions listed in one fetch.
const LIST_LIMIT: usize = 50;
/// Maximum number of search hits shown for one query.
const SEARCH_LIMIT: usize = 20;
/// Maximum characters shown for a search-hit message body.
const CONTENT_PREVIEW_LEN: usize = 120;

/// Render the Sessions page body.
pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "sessions-title"));
    ui.label(fl!(crate::i18n::loader(), "sessions-hint"));

    // Lazy-load the session list the first time the page is shown so it
    // is not empty behind a manual refresh.
    if world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| !s.0.sessions_loaded)
    {
        refresh_sessions(ai, world, ui_entity, false);
    }

    render_search(ui, ai, world, ui_entity);
    ui.separator();
    render_import(ui, ai, world, ui_entity);
    ui.separator();
    render_list(ui, ai, world, ui_entity);

    if let Some(message) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.session_message.clone())
    {
        ui.separator();
        ui.label(message);
    }
}

/// Search section: a query field plus a button that runs a message
/// search and caches the hits.
fn render_search(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.label(fl!(crate::i18n::loader(), "sessions-search-title"));

    let mut do_search = false;
    ui.horizontal(|ui| {
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
    });

    if do_search {
        run_search(ai, world, ui_entity);
    }

    let rows = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.session_search_rows.clone())
        .unwrap_or_default();

    if rows.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "sessions-search-empty"));
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
    ui.group(|ui| {
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

/// Import section: a path field plus a button that reads the file and
/// imports its JSON contents.
fn render_import(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.label(fl!(crate::i18n::loader(), "sessions-import-title"));

    let mut do_import = false;
    ui.horizontal(|ui| {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.add(
                egui::TextEdit::singleline(&mut state.0.session_import_path)
                    .desired_width(240.0)
                    .hint_text(fl!(crate::i18n::loader(), "sessions-import-placeholder")),
            );
        }
        if ui
            .button(fl!(crate::i18n::loader(), "sessions-import"))
            .clicked()
        {
            do_import = true;
        }
    });

    if do_import {
        run_import(ai, world, ui_entity);
    }
}

/// Session-list section: refreshable rows with per-row archive /
/// unarchive and export actions.
fn render_list(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.horizontal(|ui| {
        ui.label(fl!(crate::i18n::loader(), "sessions-list-title"));
        if ui
            .button(fl!(crate::i18n::loader(), "sessions-refresh"))
            .clicked()
        {
            refresh_sessions(ai, world, ui_entity, true);
        }
    });

    // Toggling "show archived" re-fetches with the new flag.
    let mut toggled = false;
    ui.horizontal(|ui| {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
            && ui
                .checkbox(
                    &mut state.0.session_show_archived,
                    fl!(crate::i18n::loader(), "sessions-show-archived"),
                )
                .changed()
        {
            toggled = true;
        }
    });
    if toggled {
        refresh_sessions(ai, world, ui_entity, false);
    }

    let sessions = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.session_rows.clone())
        .unwrap_or_default();

    if sessions.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "sessions-empty"));
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("session_list_scroll")
        .max_height(300.0)
        .show(ui, |ui| {
            for session in &sessions {
                render_session_row(ui, ai, world, ui_entity, session);
                ui.add_space(4.0);
            }
        });
}

fn render_session_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    session: &ene_store::SessionMeta,
) {
    ui.group(|ui| {
        let title = if session.title.is_empty() {
            fl!(crate::i18n::loader(), "sessions-untitled")
        } else {
            session.title.clone()
        };
        ui.label(format!(
            "{}: {}  |  {}: {}",
            fl!(crate::i18n::loader(), "sessions-col-session"),
            session.session_id,
            fl!(crate::i18n::loader(), "sessions-col-title"),
            title,
        ));
        ui.label(format!(
            "{}: {}  |  {}: {}",
            fl!(crate::i18n::loader(), "sessions-col-turns"),
            session.turn_count,
            fl!(crate::i18n::loader(), "sessions-col-updated"),
            session.updated_at.format("%Y-%m-%d %H:%M:%S UTC"),
        ));
        if session.archived {
            ui.weak(fl!(crate::i18n::loader(), "sessions-archived-badge"));
        }
        ui.horizontal(|ui| {
            if session.archived {
                if ui
                    .button(fl!(crate::i18n::loader(), "sessions-unarchive"))
                    .clicked()
                {
                    set_archived(ai, world, ui_entity, &session.session_id, false);
                }
            } else if ui
                .button(fl!(crate::i18n::loader(), "sessions-archive"))
                .clicked()
            {
                set_archived(ai, world, ui_entity, &session.session_id, true);
            }
            if ui
                .button(fl!(crate::i18n::loader(), "sessions-export"))
                .clicked()
            {
                export_session(ai, world, ui_entity, &session.session_id);
            }
        });
    });
}

/// Re-fetch the session list from the actor. When `announce` is `true`
/// (manual refresh) a success message is shown; silent refreshes after
/// an archive / import leave the action's own message in place.
fn refresh_sessions(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, announce: bool) {
    let include_archived = world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.session_show_archived);
    match ai.list_sessions_blocking(include_archived, LIST_LIMIT) {
        Ok(sessions) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.session_rows = sessions;
                state.0.sessions_loaded = true;
                if announce {
                    state.0.session_message =
                        Some(fl!(crate::i18n::loader(), "sessions-refresh-ok"));
                }
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.sessions_loaded = true;
                state.0.session_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "sessions-refresh-error")
                ));
            }
        }
    }
}

fn run_search(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    let query = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.session_search_query.clone())
        .unwrap_or_default();
    if query.trim().is_empty() {
        set_message(
            world,
            ui_entity,
            fl!(crate::i18n::loader(), "sessions-search-query-required"),
        );
        return;
    }
    match ai.search_sessions_blocking(query, SEARCH_LIMIT, 0) {
        Ok(hits) => {
            let rows = hits
                .into_iter()
                .map(|(session_id, message)| SessionSearchRow {
                    session_id,
                    role: message.role,
                    content: truncate_preview(&message.content),
                })
                .collect();
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.session_search_rows = rows;
                state.0.session_message = Some(fl!(crate::i18n::loader(), "sessions-search-ok"));
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.session_search_rows.clear();
                state.0.session_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "sessions-search-error")
                ));
            }
        }
    }
}

fn run_import(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    let path = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.session_import_path.clone())
        .unwrap_or_default();
    if path.trim().is_empty() {
        set_message(
            world,
            ui_entity,
            fl!(crate::i18n::loader(), "sessions-import-path-required"),
        );
        return;
    }
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) => {
            set_error(
                world,
                ui_entity,
                &fl!(crate::i18n::loader(), "sessions-import-read-error"),
                &error.to_string(),
            );
            return;
        }
    };
    let result = ai.import_session_blocking(json).map(|_| ());
    set_action_result(
        world,
        ui_entity,
        result,
        &fl!(crate::i18n::loader(), "sessions-import-ok"),
        &fl!(crate::i18n::loader(), "sessions-import-error"),
    );
    refresh_sessions(ai, world, ui_entity, false);
}

fn set_archived(
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    session_id: &str,
    archived: bool,
) {
    let result = ai
        .archive_session_blocking(session_id.to_string(), archived)
        .map(|_| ());
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
    set_action_result(world, ui_entity, result, &ok_key, &error_key);
    refresh_sessions(ai, world, ui_entity, false);
}

fn export_session(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, session_id: &str) {
    let json = match ai.export_session_blocking(session_id.to_string()) {
        Ok(json) => json,
        Err(error) => {
            set_error(
                world,
                ui_entity,
                &fl!(crate::i18n::loader(), "sessions-export-error"),
                &error.to_string(),
            );
            return;
        }
    };
    match write_export(session_id, &json) {
        Ok(path) => set_message(
            world,
            ui_entity,
            format!(
                "{}: {}",
                fl!(crate::i18n::loader(), "sessions-export-ok"),
                path.display()
            ),
        ),
        Err(error) => set_error(
            world,
            ui_entity,
            &fl!(crate::i18n::loader(), "sessions-export-error"),
            &error,
        ),
    }
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

fn set_message(world: &mut World, ui_entity: Entity, message: String) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.session_message = Some(message);
    }
}

/// Record the outcome of an actor action: a localized success message on
/// `Ok`, or `"<error label>: <detail>"` on `Err`.
fn set_action_result(
    world: &mut World,
    ui_entity: Entity,
    result: Result<(), crate::ai_bridge::AiBridgeError>,
    ok_label: &str,
    error_label: &str,
) {
    let message = match result {
        Ok(()) => ok_label.to_string(),
        Err(error) => format!("{error_label}: {error}"),
    };
    set_message(world, ui_entity, message);
}

/// Record a failure as `"<error label>: <detail>"`.
fn set_error(world: &mut World, ui_entity: Entity, error_label: &str, detail: &str) {
    set_message(world, ui_entity, format!("{error_label}: {detail}"));
}
