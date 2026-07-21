//! Permission Center settings page (#177).
//!
//! Surfaces the two halves of the tool-permission model:
//!
//! * **Pending approvals** — destructive operations the actor is
//!   currently blocked on (accumulated from
//!   [`crate::event::ai::AiPermissionRequested`] into
//!   [`crate::settings::UiState::permission_requests`]). Each row offers
//!   Approve (once) / Approve (session) / Deny, which forward a
//!   [`ene_runtime::PermissionDecision`] through the [`AiBridge`].
//! * **Granted scopes** — standing session-wide grants fetched from the
//!   actor via [`AiBridge::list_permissions_blocking`], each revocable,
//!   plus a "reset all" action.
//!
//! The page mirrors the data-fetch pattern of [`super::page_memory`]:
//! blocking calls run on the UI thread through the bridge's tokio
//! handle, results are cached on [`UiStateComponent`], and a status
//! line reports the outcome of the last operation.
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_runtime::{GrantType, PermissionDecision, PermissionScope, RequestId};
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::PendingPermission;

/// Render the Permission Center page body.
pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "permissions-title"));
    ui.label(fl!(crate::i18n::loader(), "permissions-hint"));

    // Load the grants the first time the page is shown so the list is
    // not empty behind a manual refresh.
    if world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.permission_grants.is_empty())
    {
        refresh_grants(ai, world, ui_entity, false);
    }

    render_pending(ui, ai, world, ui_entity);
    ui.separator();
    render_granted(ui, ai, world, ui_entity);

    if let Some(message) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.permission_message.clone())
    {
        ui.separator();
        ui.label(message);
    }
}

/// Pending-approval section: one group per outstanding request.
fn render_pending(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.label(fl!(crate::i18n::loader(), "permissions-pending"));

    let pending = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.permission_requests.clone())
        .unwrap_or_default();

    if pending.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "permissions-pending-empty"));
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("permission_pending_scroll")
        .max_height(220.0)
        .show(ui, |ui| {
            for request in &pending {
                render_pending_row(ui, ai, world, ui_entity, request);
                ui.add_space(4.0);
            }
        });
}

fn render_pending_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    request: &PendingPermission,
) {
    ui.group(|ui| {
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
                    world,
                    ui_entity,
                    request.request_id.clone(),
                    PermissionDecision::Deny,
                );
            }
        });
    });
}

/// Granted-scope section: refreshable table with per-row revoke and a
/// reset-all action.
fn render_granted(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.horizontal(|ui| {
        ui.label(fl!(crate::i18n::loader(), "permissions-granted"));
        if ui
            .button(fl!(crate::i18n::loader(), "permissions-refresh"))
            .clicked()
        {
            refresh_grants(ai, world, ui_entity, true);
        }
    });

    let grants = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.permission_grants.clone())
        .unwrap_or_default();

    if grants.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "permissions-granted-empty"));
    } else {
        if ui
            .button(fl!(crate::i18n::loader(), "permissions-reset-all"))
            .clicked()
        {
            reset_all(ai, world, ui_entity);
        }
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("permission_granted_scroll")
            .max_height(280.0)
            .show(ui, |ui| {
                for grant in &grants {
                    render_grant_row(ui, ai, world, ui_entity, grant);
                    ui.add_space(4.0);
                }
            });
    }
}

fn render_grant_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    grant: &PermissionScope,
) {
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
            revoke(ai, world, ui_entity, grant.id);
        }
    });
}

/// Forward a decision, drop the request from the pending list, then
/// refresh the granted scopes (an `AllowSession` decision creates one).
fn decide(
    ai: &Arc<AiBridge>,
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
    set_message(world, ui_entity, result);
    refresh_grants(ai, world, ui_entity, false);
}

fn revoke(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, id: u64) {
    let result = ai.revoke_permission_blocking(id).map(|_| ());
    set_message(world, ui_entity, result);
    refresh_grants(ai, world, ui_entity, false);
}

fn reset_all(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    let result = ai.reset_all_permissions_blocking().map(|_| ());
    set_message(world, ui_entity, result);
    refresh_grants(ai, world, ui_entity, false);
}

/// Re-fetch the standing grants from the actor. When `announce` is
/// `true` (manual refresh) a success message is shown; silent
/// refreshes after a decision/revocation leave the action's own
/// message in place.
fn refresh_grants(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, announce: bool) {
    match ai.list_permissions_blocking() {
        Ok(grants) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.permission_grants = grants;
                if announce {
                    state.0.permission_message =
                        Some(fl!(crate::i18n::loader(), "permissions-refresh-ok"));
                }
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.permission_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "permissions-refresh-error")
                ));
            }
        }
    }
}

fn set_message(world: &mut World, ui_entity: Entity, result: Result<(), String>) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.permission_message = Some(match result {
            Ok(()) => fl!(crate::i18n::loader(), "permissions-action-ok"),
            Err(error) => format!(
                "{}: {error}",
                fl!(crate::i18n::loader(), "permissions-action-error")
            ),
        });
    }
}

fn grant_type_label(grant_type: GrantType) -> String {
    match grant_type {
        GrantType::Once => fl!(crate::i18n::loader(), "permissions-grant-once"),
        GrantType::Session => fl!(crate::i18n::loader(), "permissions-grant-session"),
        GrantType::Permanent => fl!(crate::i18n::loader(), "permissions-grant-permanent"),
    }
}

fn field_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.add(
        egui::Label::new(format!("{label}: {value}"))
            .wrap()
            .selectable(true),
    );
}
