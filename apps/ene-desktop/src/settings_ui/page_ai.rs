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
    _settings: &mut CharacterSettings,
    draft: &mut SettingsDraft,
    _animation: &mut crate::character_state::AnimationControl,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-core-hint"));
    input.core_settings.poll();
    if !input.core_settings.started() {
        input.core_settings.start(ai.fetch_core_settings());
    }
    if let Some(Ok(settings)) = &input.core_settings.data {
        seed_draft_once(draft, settings);
    }

    section_card(
        ui,
        "ai-chat",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-chat"),
        |ui| {
            let value = draft
                .editing()
                .section_value("ai")
                .unwrap_or_else(|| serde_json::json!({}));
            let mut text =
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-json-hint"));
            if ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(16),
                )
                .changed()
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text)
            {
                draft.set_section_value("ai", parsed);
            }
        },
    );
}

fn seed_draft_once(draft: &mut SettingsDraft, settings: &serde_json::Value) {
    if draft.editing().section_value("ai").is_some() {
        return;
    }
    if let Some(ai) = settings.get("ai") {
        draft.seed_core_section("ai", ai.clone());
    }
}
