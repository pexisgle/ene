use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::{empty_state, section_card};
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;

pub fn render(
    ui: &mut egui::Ui,
    _settings: &CharacterSettings,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    ui.weak(fl!(crate::i18n::loader(), "permissions-core-hint"));
    input.approvals.poll();
    if !input.approvals.started() {
        input.approvals.start(ai.fetch_approvals());
    }
    section_card(
        ui,
        "permissions-pending",
        &fl!(crate::i18n::loader(), "permissions-pending"),
        |ui| {
            if ui.button(fl!(crate::i18n::loader(), "refresh")).clicked() {
                input.approvals.restart(ai.fetch_approvals());
            }
            let Some(items) = input.approvals.data.clone() else {
                empty_state(ui, &fl!(crate::i18n::loader(), "permissions-loading"), "");
                return;
            };
            if items.is_empty() {
                empty_state(
                    ui,
                    &fl!(crate::i18n::loader(), "permissions-pending-empty"),
                    "",
                );
                return;
            }
            for approval in items {
                ui.separator();
                ui.label(format!("{} → {}", approval.tool, approval.target));
                ui.horizontal(|ui| {
                    if ui
                        .button(fl!(crate::i18n::loader(), "permissions-approve-once"))
                        .clicked()
                    {
                        drop(ai.respond_approval(approval.id.clone(), "allow".to_owned()));
                        input.approvals.restart(ai.fetch_approvals());
                    }
                    if ui
                        .button(fl!(crate::i18n::loader(), "permissions-deny"))
                        .clicked()
                    {
                        drop(ai.respond_approval(approval.id.clone(), "deny".to_owned()));
                        input.approvals.restart(ai.fetch_approvals());
                    }
                });
            }
        },
    );
}
