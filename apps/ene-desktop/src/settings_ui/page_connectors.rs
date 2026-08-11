//! Connectors settings page.
//!
//! Read-only status surface for the connector framework: lists the
//! registered connectors, and for the selected connector shows health,
//! accounts, standing per-action grants, and a connectivity-check button.
//! All fetches run on the bridge runtime through [`AsyncData`] receivers so
//! the render thread never blocks.
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_connector::{ConnectionState, ConnectorId};
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings_ui::components::{
    BadgeTone, empty_state, section_card, setting_row, status_badge,
};
use crate::settings_ui::input::{AsyncData, SettingsInputState};

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    input.connectors.poll();
    if !input.connectors.started() {
        input.connectors.start(ai.fetch_connectors());
    }
    input.connector_detail.poll();
    input.connector_check.poll();
    if let Some(selected) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_selected.clone())
        && !input.connector_detail.started()
    {
        input
            .connector_detail
            .start(ai.fetch_connector_detail(selected));
    }

    ui.vertical(|ui| {
        section_card(
            ui,
            "connectors-list",
            &fl!(crate::i18n::loader(), "connectors-list"),
            |ui| render_list(ui, ai, input, world, ui_entity),
        );
        section_card(
            ui,
            "connectors-detail",
            &fl!(crate::i18n::loader(), "connectors-status-title"),
            |ui| render_selected(ui, ai, input, world, ui_entity),
        );
    });

    input.connector_check.poll();
    if let Some(result) = input.connector_check.data.take() {
        ui.add_space(6.0);
        ui.label(match result {
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
        "connectors_refresh_row",
        &fl!(crate::i18n::loader(), "connectors-hint"),
        "",
        |ui| {
            if ui
                .button(fl!(crate::i18n::loader(), "connectors-refresh"))
                .clicked()
            {
                input.connectors = AsyncData::new();
            }
        },
    );
    if input.connectors.loading() {
        ui.weak(fl!(crate::i18n::loader(), "connectors-loading"));
        return;
    }
    if let Some(error) = input.connectors.error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
        return;
    }
    let summaries = input.connectors.data.clone().unwrap_or_default();

    if summaries.is_empty() {
        empty_state(ui, &fl!(crate::i18n::loader(), "connectors-list-empty"), "");
        return;
    }

    let selected = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_selected.clone());
    for summary in &summaries {
        let id = summary.identity.id.clone();
        let label = format!(
            "{} — {} ({} {})",
            id,
            summary.identity.display_name,
            summary.account_count,
            fl!(crate::i18n::loader(), "connectors-accounts-label")
        );
        ui.horizontal(|ui| {
            status_badge(
                ui,
                &connection_label(&summary.connection),
                connection_tone(&summary.connection),
            );
            if ui
                .selectable_label(selected.as_ref() == Some(&id), label)
                .clicked()
            {
                select(ai, input, world, ui_entity, id);
            }
        });
    }
}

fn render_selected(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let Some(selected) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.connector_selected.clone())
    else {
        empty_state(
            ui,
            &fl!(crate::i18n::loader(), "connectors-none-selected"),
            "",
        );
        return;
    };

    setting_row(
        ui,
        "connectors_selected_row",
        &selected.to_string(),
        "",
        |ui| {
            if ui
                .button(fl!(crate::i18n::loader(), "connectors-check"))
                .clicked()
            {
                input.connector_check = AsyncData::new();
                input
                    .connector_check
                    .start(ai.apply_connector_check(selected.clone()));
            }
        },
    );

    if input.connector_detail.loading() {
        ui.weak(fl!(crate::i18n::loader(), "connectors-loading"));
        return;
    }
    let Some((status, grants)) = input.connector_detail.data.clone() else {
        return;
    };
    if let Some(status) = status {
        if let Some(health) = &status.health {
            let health_label = if health.healthy {
                fl!(crate::i18n::loader(), "connectors-healthy")
            } else {
                fl!(crate::i18n::loader(), "connectors-unhealthy")
            };
            setting_row(
                ui,
                "connectors_health_row",
                &fl!(crate::i18n::loader(), "connectors-health"),
                health.message.as_deref().unwrap_or("-"),
                |ui| {
                    status_badge(
                        ui,
                        &health_label,
                        if health.healthy {
                            BadgeTone::Ok
                        } else {
                            BadgeTone::Error
                        },
                    );
                },
            );
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
    if grants.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "connectors-grants-empty"));
    } else {
        egui::ScrollArea::vertical()
            .id_salt("connector_grants_scroll")
            .max_height(180.0)
            .show(ui, |ui| {
                for grant in &grants {
                    egui::Frame::group(ui.style())
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
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

fn select(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    id: ConnectorId,
) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.connector_selected = Some(id.clone());
    }
    input.connector_detail = AsyncData::new();
    input.connector_detail.start(ai.fetch_connector_detail(id));
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

fn connection_tone(connection: &ConnectionState) -> BadgeTone {
    match connection {
        ConnectionState::Disconnected => BadgeTone::Warn,
        ConnectionState::Connected { .. } => BadgeTone::Ok,
        ConnectionState::Error { .. } => BadgeTone::Error,
    }
}

fn field_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.monospace(value);
    });
}
