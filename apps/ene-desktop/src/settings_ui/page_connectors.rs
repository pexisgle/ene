//! Connectors settings page.
//!
//! Read-only status surface for the connector framework: lists the
//! registered connectors with their cached connection state, and for the
//! selected connector shows health, accounts, standing per-action grants,
//! and a connectivity-check button. Mirrors the data-fetch pattern of
//! [`super::page_permissions`]: blocking calls run on the UI thread through
//! the bridge's tokio handle and results are cached on `UiStateComponent`.

use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_connector::{ConnectionState, ConnectorId};
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;

pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "connectors-title"));
    ui.label(fl!(crate::i18n::loader(), "connectors-hint"));

    if world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| !s.0.connectors_loaded)
    {
        refresh_list(ai, world, ui_entity);
    }

    render_list(ui, ai, world, ui_entity);
    ui.separator();
    render_selected(ui, ai, world, ui_entity);

    if let Some(message) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_message.clone())
    {
        ui.separator();
        ui.label(message);
    }
}

fn render_list(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.horizontal(|ui| {
        ui.label(fl!(crate::i18n::loader(), "connectors-list"));
        if ui
            .button(fl!(crate::i18n::loader(), "connectors-refresh"))
            .clicked()
        {
            refresh_list(ai, world, ui_entity);
        }
    });

    let summaries = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.connector_summaries.clone())
        .unwrap_or_default();

    if summaries.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "connectors-list-empty"));
        return;
    }

    let selected = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_selected.clone());
    for summary in &summaries {
        let id = summary.identity.id.clone();
        let label = format!(
            "{} — {} ({}, {} {})",
            id,
            summary.identity.display_name,
            connection_label(&summary.connection),
            summary.account_count,
            fl!(crate::i18n::loader(), "connectors-accounts-label")
        );
        if ui
            .selectable_label(selected.as_ref() == Some(&id), label)
            .clicked()
        {
            select(ai, world, ui_entity, id);
        }
    }
}

fn render_selected(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    let Some(selected) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_selected.clone())
    else {
        ui.weak(fl!(crate::i18n::loader(), "connectors-none-selected"));
        return;
    };

    ui.horizontal(|ui| {
        ui.label(format!(
            "{} {}",
            fl!(crate::i18n::loader(), "connectors-status-title"),
            selected
        ));
        if ui
            .button(fl!(crate::i18n::loader(), "connectors-check"))
            .clicked()
        {
            check(ai, world, ui_entity, &selected);
        }
    });

    let status = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_status.clone());
    if let Some(status) = status {
        if let Some(health) = &status.health {
            ui.label(format!(
                "{}: {} ({})",
                fl!(crate::i18n::loader(), "connectors-health"),
                if health.healthy {
                    fl!(crate::i18n::loader(), "connectors-healthy")
                } else {
                    fl!(crate::i18n::loader(), "connectors-unhealthy")
                },
                health.message.as_deref().unwrap_or("-")
            ));
        }
        if status.accounts.is_empty() {
            ui.weak(fl!(crate::i18n::loader(), "connectors-accounts-empty"));
        } else {
            ui.label(fl!(crate::i18n::loader(), "connectors-accounts"));
            for account in &status.accounts {
                ui.label(format!("  - {} ({:?})", account.label, account.auth));
            }
        }
    }

    ui.add_space(4.0);
    ui.label(fl!(crate::i18n::loader(), "connectors-grants"));
    let grants = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.connector_grants.clone())
        .unwrap_or_default();
    if grants.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "connectors-grants-empty"));
    } else {
        egui::ScrollArea::vertical()
            .id_salt("connector_grants_scroll")
            .max_height(180.0)
            .show(ui, |ui| {
                for grant in &grants {
                    ui.group(|ui| {
                        field_label(
                            ui,
                            &fl!(crate::i18n::loader(), "action-label"),
                            &grant.action,
                        );
                        field_label(
                            ui,
                            &fl!(crate::i18n::loader(), "permissions-target-pattern"),
                            &grant.target_pattern,
                        );
                        ui.label(format!(
                            "{}: {}",
                            fl!(crate::i18n::loader(), "permissions-granted-at"),
                            grant.granted_at.format("%Y-%m-%d %H:%M:%S UTC")
                        ));
                    });
                    ui.add_space(4.0);
                }
            });
    }
}

fn refresh_list(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    let summaries = ai.list_connectors_blocking();
    let selected = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_selected.clone());
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.connector_summaries = summaries;
        state.0.connectors_loaded = true;
    }
    // A refresh may have removed the selected connector (unregister).
    let Some(selected) = selected else {
        return;
    };
    let still_registered = world.get::<UiStateComponent>(ui_entity).is_some_and(|s| {
        s.0.connector_summaries
            .iter()
            .any(|summary| summary.identity.id == selected)
    });
    if !still_registered && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.connector_selected = None;
        state.0.connector_status = None;
        state.0.connector_grants = Vec::new();
    }
}

fn select(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, id: ConnectorId) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.connector_selected = Some(id.clone());
    }
    refresh_selected(ai, world, ui_entity, &id);
}

fn refresh_selected(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, id: &ConnectorId) {
    let status = ai.connector_status_blocking(id);
    let grants = ai.connector_permissions_blocking(id);
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.connector_status = status.ok();
        state.0.connector_grants = grants.unwrap_or_default();
    }
}

fn check(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, id: &ConnectorId) {
    let result = ai.check_connector_blocking(id);
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.connector_message = Some(match result {
            Ok(health) if health.healthy => {
                fl!(crate::i18n::loader(), "connectors-check-ok")
            }
            Ok(health) => format!(
                "{}: {}",
                fl!(crate::i18n::loader(), "connectors-check-failed"),
                health.message.as_deref().unwrap_or("-")
            ),
            Err(error) => format!(
                "{}: {error}",
                fl!(crate::i18n::loader(), "connectors-check-error")
            ),
        });
    }
    refresh_selected(ai, world, ui_entity, id);
}

fn connection_label(connection: &ConnectionState) -> String {
    match connection {
        ConnectionState::Disconnected => {
            fl!(crate::i18n::loader(), "connectors-disconnected")
        }
        ConnectionState::Connected { .. } => {
            fl!(crate::i18n::loader(), "connectors-connected")
        }
        ConnectionState::Error { .. } => fl!(crate::i18n::loader(), "connectors-error"),
    }
}

fn field_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.add(
        egui::Label::new(format!("{label}: {value}"))
            .wrap()
            .selectable(true),
    );
}
