use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

pub fn render(
    ui: &mut egui::Ui,
    _settings: &CharacterSettings,
    _draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    input.approvals.poll();
    if !input.approvals.started() {
        input.approvals.start(ai.fetch_approvals());
    }
    section_card(
        ui,
        "approvals-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "approvals"),
        |ui| {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "refresh"))
                .clicked()
            {
                input.approvals.restart(ai.fetch_approvals());
            }
            let Some(items) = input.approvals.data.clone() else {
                return;
            };
            for approval in items {
                ui.separator();
                ui.label(format!("{} → {}", approval.tool, approval.target));
                ui.horizontal(|ui| {
                    if ui.button("allow").clicked() {
                        drop(ai.respond_approval(approval.id.clone(), "allow".to_owned()));
                        input.approvals.restart(ai.fetch_approvals());
                    }
                    if ui.button("deny").clicked() {
                        drop(ai.respond_approval(approval.id.clone(), "deny".to_owned()));
                        input.approvals.restart(ai.fetch_approvals());
                    }
                });
            }
        },
    );
}
