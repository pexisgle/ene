//! Permission Center settings page.
//!
//! * **Pending approvals** — destructive operations the actor is blocked on
//!   (from [`crate::event::ai::AiPermissionRequested`] into
//!   [`crate::settings::UiState::permission_requests`]); each row offers
//!   Approve (once) / Approve (session) / Deny.
//! * **Granted scopes** — standing session-wide grants, loaded
//!   asynchronously (never blocking the render thread), each revocable,
//!   plus a "reset all" action.
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_runtime::{GrantType, PermissionDecision, PermissionScope, RequestId};
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::PendingPermission;
use crate::settings_ui::components::{danger_button, empty_state, section_card, setting_row};
use crate::settings_ui::input::{AsyncData, SettingsInputState};

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    input.permissions.poll();
    if !input.permissions.started() {
        input.permissions.start(ai.fetch_permissions());
    }
    input.permission_action.poll();
    if let Some(message) = input.permission_action.data.take() {
        ui.label(message);
        input.permissions = AsyncData::new();
    }

    ui.vertical(|ui| {
        section_card(
            ui,
            "permissions-pending",
            &fl!(crate::i18n::loader(), "permissions-pending"),
            |ui| render_pending(ui, ai, input, world, ui_entity),
        );
        section_card(
            ui,
            "permissions-grants",
            &fl!(crate::i18n::loader(), "permissions-granted"),
            |ui| render_granted(ui, ai, input),
        );
    });
}

fn render_pending(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let pending = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.permission_requests.clone())
        .unwrap_or_default();

    if pending.is_empty() {
        empty_state(
            ui,
            &fl!(crate::i18n::loader(), "permissions-pending-empty"),
            "",
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("permission_pending_scroll")
        .max_height(220.0)
        .show(ui, |ui| {
            for request in &pending {
                render_pending_row(ui, ai, input, world, ui_entity, request);
                ui.add_space(4.0);
            }
        });
}

fn render_pending_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    request: &PendingPermission,
) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            field_label(
                ui,
                &fl!(crate::i18n::loader(), "action-label"),
                &request.action,
            );
            field_label(
                ui,
                &fl!(crate::i18n::loader(), "target-label"),
                &request.target,
            );
            if let Some(description) = &request.description {
                field_label(
                    ui,
                    &fl!(crate::i18n::loader(), "description-label"),
                    description,
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .button(fl!(crate::i18n::loader(), "permissions-approve-once"))
                    .clicked()
                {
                    decide(
                        ai,
                        input,
                        world,
                        ui_entity,
                        request.request_id.clone(),
                        PermissionDecision::AllowOnce,
                    );
                }
                if ui
                    .button(fl!(crate::i18n::loader(), "permissions-approve-session"))
                    .clicked()
                {
                    decide(
                        ai,
                        input,
                        world,
                        ui_entity,
                        request.request_id.clone(),
                        PermissionDecision::AllowSession,
                    );
                }
                if ui
                    .button(fl!(crate::i18n::loader(), "permissions-deny"))
                    .clicked()
                {
                    decide(
                        ai,
                        input,
                        world,
                        ui_entity,
                        request.request_id.clone(),
                        PermissionDecision::Deny,
                    );
                }
            });
        });
}

/// Forward a decision (a non-blocking channel send), drop the request from
/// the pending list, and refresh the granted scopes asynchronously.
fn decide(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    request_id: RequestId,
    decision: PermissionDecision,
) {
    let result = ai.answer_permission(request_id.clone(), decision);
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state
            .0
            .permission_requests
            .retain(|p| p.request_id != request_id);
    }
    if let Err(error) = result {
        start_action_message(input, error.to_string());
    }
    input.permissions = AsyncData::new();
}

fn revoke(ai: &Arc<AiBridge>, input: &mut SettingsInputState, id: u64) {
    input.permissions = AsyncData::new();
    let receiver = ai.apply_revoke_permission(id);
    input.permission_action = AsyncData::new();
    input.permission_action.start(spawn_message(
        ai,
        receiver,
        fl!(crate::i18n::loader(), "permissions-revoke-ok"),
        fl!(crate::i18n::loader(), "permissions-revoke-error"),
    ));
}

fn reset_all(ai: &Arc<AiBridge>, input: &mut SettingsInputState) {
    input.permissions = AsyncData::new();
    let receiver = ai.apply_reset_permissions();
    input.permission_action = AsyncData::new();
    input.permission_action.start(spawn_message(
        ai,
        receiver,
        fl!(crate::i18n::loader(), "permissions-reset-ok"),
        fl!(crate::i18n::loader(), "permissions-reset-error"),
    ));
}

fn start_action_message(input: &mut SettingsInputState, message: String) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if tx.send(message).is_err() {
        tracing::debug!(component = "PermissionsPage", "action message dropped");
    }
    input.permission_action = AsyncData::new();
    input.permission_action.start(rx);
}

fn spawn_message<T: std::fmt::Display + Send + 'static>(
    ai: &AiBridge,
    receiver: tokio::sync::oneshot::Receiver<Result<T, String>>,
    ok_label: String,
    error_label: String,
) -> tokio::sync::oneshot::Receiver<String> {
    ai.spawn_fetch(async move {
        match receiver.await {
            Ok(Ok(_)) => ok_label,
            Ok(Err(error)) => format!("{error_label}: {error}"),
            Err(_) => error_label,
        }
    })
}

/// Granted-scope section: refreshable table with per-row revoke and a
/// reset-all action. The list is fetched asynchronously.
fn render_granted(ui: &mut egui::Ui, ai: &Arc<AiBridge>, input: &mut SettingsInputState) {
    setting_row(
        ui,
        "permissions_refresh_row",
        &fl!(crate::i18n::loader(), "permissions-hint"),
        "",
        |ui| {
            if ui
                .button(fl!(crate::i18n::loader(), "permissions-refresh"))
                .clicked()
            {
                input.permissions = AsyncData::new();
            }
        },
    );

    if input.permissions.loading() {
        ui.weak(fl!(crate::i18n::loader(), "permissions-loading"));
        return;
    }
    if let Some(error) = input.permissions.error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
        if ui
            .small_button(fl!(crate::i18n::loader(), "permissions-retry"))
            .clicked()
        {
            input.permissions = AsyncData::new();
        }
        return;
    }

    let grants = input.permissions.data.clone().unwrap_or_default();
    if grants.is_empty() {
        empty_state(
            ui,
            &fl!(crate::i18n::loader(), "permissions-granted-empty"),
            "",
        );
    } else {
        if danger_button(ui, &fl!(crate::i18n::loader(), "permissions-reset-all")).clicked() {
            reset_all(ai, input);
        }
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("permission_granted_scroll")
            .max_height(280.0)
            .show(ui, |ui| {
                for grant in &grants {
                    render_grant_row(ui, ai, input, grant);
                    ui.add_space(4.0);
                }
            });
    }
}

fn render_grant_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    grant: &PermissionScope,
) {
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
                "{}: {}  |  {}: {}",
                fl!(crate::i18n::loader(), "permissions-grant-type"),
                grant_type_label(grant.grant_type),
                fl!(crate::i18n::loader(), "permissions-granted-at"),
                grant.granted_at.format("%Y-%m-%d %H:%M:%S UTC"),
            ));
            if ui
                .button(fl!(crate::i18n::loader(), "permissions-revoke"))
                .clicked()
            {
                revoke(ai, input, grant.id);
            }
        });
}

fn grant_type_label(grant_type: GrantType) -> String {
    match grant_type {
        GrantType::Once => fl!(crate::i18n::loader(), "permissions-grant-once"),
        GrantType::Session => fl!(crate::i18n::loader(), "permissions-grant-session"),
        GrantType::Permanent => fl!(crate::i18n::loader(), "permissions-grant-permanent"),
    }
}

fn field_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.monospace(value);
    });
}
