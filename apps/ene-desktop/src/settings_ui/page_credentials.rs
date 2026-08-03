//! Credentials settings page (interim).
//!
//! Lists stored credentials (id / kind / expiry / status) plus every
//! currently declared `OAuth2` credential, and offers authorize / revoke
//! actions. This is a stop-gap page until the schema-driven generic settings
//! UI lands; it deliberately reuses the same lazy-load + message pattern as
//! [`super::page_permissions`].
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_plugin_host::oauth::{CredentialInfo, CredentialKindName};
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;

pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "credentials-title"));
    ui.label(fl!(crate::i18n::loader(), "credentials-hint"));

    // Load the list the first time the page is shown so it is not empty
    // behind a manual refresh.
    if world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.credential_rows.is_empty())
    {
        refresh_rows(ai, world, ui_entity, false);
    }

    ui.horizontal(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "credentials-refresh"))
            .clicked()
        {
            refresh_rows(ai, world, ui_entity, true);
        }
    });

    let rows = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.credential_rows.clone())
        .unwrap_or_default();

    if rows.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "credentials-empty"));
    } else {
        egui::ScrollArea::vertical()
            .id_salt("credential_rows_scroll")
            .max_height(360.0)
            .show(ui, |ui| {
                for row in &rows {
                    render_row(ui, ai, world, ui_entity, row);
                    ui.add_space(4.0);
                }
            });
    }

    if let Some(message) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.credential_message.clone())
    {
        ui.separator();
        ui.label(message);
    }
}

fn render_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    row: &CredentialInfo,
) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(row.id.as_str()).strong());
            ui.weak(credential_kind_label(row));
        });
        let status = match (row.stored, row.expired) {
            (true, true) => fl!(crate::i18n::loader(), "credentials-status-expired"),
            (true, false) => fl!(crate::i18n::loader(), "credentials-status-ok"),
            (false, _) => fl!(crate::i18n::loader(), "credentials-status-missing"),
        };
        ui.label(format!(
            "{}  |  {}",
            status,
            row.expires_at.map_or_else(
                || fl!(crate::i18n::loader(), "credentials-expiry-none").to_string(),
                |t| format!("{} UTC", t.format("%Y-%m-%d %H:%M:%S"))
            )
        ));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    row.stored,
                    egui::Button::new(fl!(crate::i18n::loader(), "credentials-revoke")),
                )
                .clicked()
            {
                revoke(ai, world, ui_entity, row.id.clone());
            }
            if ui
                .add_enabled(
                    matches!(row.kind, CredentialKindName::OAuth2) && row.shared,
                    egui::Button::new(fl!(crate::i18n::loader(), "credentials-authorize")),
                )
                .on_disabled_hover_text(fl!(
                    crate::i18n::loader(),
                    "credentials-authorize-private-only"
                ))
                .clicked()
            {
                authorize(ai, world, ui_entity, row.id.clone());
            }
        });
    });
}

fn credential_kind_label(row: &CredentialInfo) -> String {
    match row.kind {
        CredentialKindName::OAuth2 => fl!(crate::i18n::loader(), "credentials-kind-oauth2"),
        CredentialKindName::ApiKey => fl!(crate::i18n::loader(), "credentials-kind-api-key"),
        CredentialKindName::None => fl!(crate::i18n::loader(), "credentials-kind-none"),
    }
}

fn authorize(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, id: String) {
    let result = ai.authorize_credential_blocking(id);
    set_message(world, ui_entity, result);
}

fn revoke(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, id: String) {
    let result = ai.revoke_credential_blocking(vec![id]).map(|_| ());
    set_message(world, ui_entity, result);
    refresh_rows(ai, world, ui_entity, false);
}

/// Re-fetch the credential rows from the actor. When `announce` is `true`
/// (manual refresh) a success message is shown; silent refreshes after an
/// action leave the action's own message in place.
fn refresh_rows(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, announce: bool) {
    match ai.list_credentials_blocking() {
        Ok(rows) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.credential_rows = rows;
                if announce {
                    state.0.credential_message =
                        Some(fl!(crate::i18n::loader(), "credentials-refresh-ok"));
                }
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.credential_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "credentials-refresh-error")
                ));
            }
        }
    }
}

fn set_message(
    world: &mut World,
    ui_entity: Entity,
    result: Result<(), crate::ai_bridge::AiBridgeError>,
) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.credential_message = Some(match result {
            Ok(()) => fl!(crate::i18n::loader(), "credentials-action-ok"),
            Err(error) => format!(
                "{}: {error}",
                fl!(crate::i18n::loader(), "credentials-action-error")
            ),
        });
    }
}
