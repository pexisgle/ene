use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::{section_card, toggle_row};
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use crate::component::ui::UiStateComponent;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};

pub fn render_config(
    ui: &mut egui::Ui,
    _settings: &CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
) {
    input.core_settings.poll();
    if !input.core_settings.started() {
        input.core_settings.start(ai.fetch_core_settings());
    }
    if let Some(Ok(settings)) = &input.core_settings.data
        && draft.editing().section_value("mind").is_none()
        && let Some(mind) = settings.pointer("/effective/mind")
    {
        draft.seed_core_section("mind", mind.clone());
    }

    section_card(
        ui,
        "memory-config",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memory-config-storage"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "memory-core-hint"
            ));
            number_u64(ui, draft, "/recall/budget", "memory-recall-budget");
            number_f32(
                ui,
                draft,
                "/recall/weight_lexical",
                "memory-recall-weight-lexical",
            );
            number_f32(
                ui,
                draft,
                "/recall/weight_recency",
                "memory-recall-weight-recency",
            );
            number_f32(
                ui,
                draft,
                "/recall/weight_salience",
                "memory-recall-weight-salience",
            );
            number_f32(
                ui,
                draft,
                "/recall/weight_embedding",
                "memory-recall-weight-embedding",
            );
        },
    );
    section_card(
        ui,
        "memory-approval",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memory-config-approval"),
        |ui| {
            let mut require = bool_at(draft, "/memory_approval/require_approval");
            if toggle_row(
                ui,
                "memory-require-approval",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "memory-config-require-approval"),
                "",
                &mut require,
            ) {
                set_path(draft, "/memory_approval/require_approval", json!(require));
            }
            number_f32(
                ui,
                draft,
                "/memory_approval/confidence_threshold",
                "memory-confidence-threshold",
            );
            number_f32(
                ui,
                draft,
                "/memory_approval/shared_confidence_threshold",
                "memory-shared-confidence-threshold",
            );
        },
    );
    section_card(
        ui,
        "memory-limits",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memory-config-limits"),
        |ui| {
            number_f32(
                ui,
                draft,
                "/forgetting/salience_threshold",
                "memory-forget-salience",
            );
        },
    );
}

pub fn render_journal(
    ui: &mut egui::Ui,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    input.memories.poll();
    if !input.memories.started() {
        input.memories.start(ai.fetch_memories());
    }
    let pending = world
        .get::<UiStateComponent>(ui_entity)
        .map_or(0, |state| state.0.memory_journal_pending_count);
    section_card(
        ui,
        "memory-journal",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "memories"),
        |ui| {
            if pending > 0 {
                ui.label(format!(
                    "{}: {pending}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "memory-page-tab-pending")
                ));
            }
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "refresh"))
                .clicked()
            {
                input.memories.restart(ai.fetch_memories());
            }
            if input.memories.loading() {
                ui.weak("…");
            }
            if let Some(err) = &input.memories.error {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x8a, 0x65), err);
            }
            let Some(items) = input.memories.data.clone() else {
                return;
            };
            for item in items {
                ui.separator();
                ui.strong(&item.title);
                ui.label(format!("{} · {}", item.kind, item.scope));
                ui.label(&item.content);
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "delete"))
                    .clicked()
                {
                    drop(ai.delete_memory(item.id.clone()));
                    input.memories.restart(ai.fetch_memories());
                }
            }
        },
    );
}

fn bool_at(draft: &SettingsDraft, pointer: &str) -> bool {
    draft
        .editing()
        .section_value("mind")
        .and_then(|mind| mind.pointer(pointer).and_then(Value::as_bool))
        .unwrap_or(false)
}

fn number_u64(ui: &mut egui::Ui, draft: &mut SettingsDraft, pointer: &str, label_key: &str) {
    ui.label(crate::i18n::loader().get(label_key));
    let mut text = draft
        .editing()
        .section_value("mind")
        .and_then(|mind| mind.pointer(pointer).and_then(Value::as_u64))
        .map(|n| n.to_string())
        .unwrap_or_default();
    if ui
        .add(egui::TextEdit::singleline(&mut text).desired_width(120.0))
        .changed()
        && let Ok(n) = text.trim().parse::<u64>()
    {
        set_path(draft, pointer, json!(n));
    }
}

fn number_f32(ui: &mut egui::Ui, draft: &mut SettingsDraft, pointer: &str, label_key: &str) {
    ui.label(crate::i18n::loader().get(label_key));
    let mut text = draft
        .editing()
        .section_value("mind")
        .and_then(|mind| mind.pointer(pointer).and_then(Value::as_f64))
        .map(|n| n.to_string())
        .unwrap_or_default();
    if ui
        .add(egui::TextEdit::singleline(&mut text).desired_width(120.0))
        .changed()
        && let Ok(n) = text.trim().parse::<f32>()
    {
        set_path(draft, pointer, json!(n));
    }
}

fn set_path(draft: &mut SettingsDraft, pointer: &str, value: Value) {
    let mut mind = draft
        .editing()
        .section_value("mind")
        .unwrap_or_else(|| json!({}));
    set_pointer(&mut mind, pointer, value);
    draft.set_section_value("mind", mind);
}

fn set_pointer(root: &mut Value, pointer: &str, value: Value) {
    let mut path = Vec::new();
    for part in pointer.split('/').filter(|part| !part.is_empty()) {
        path.push(part);
    }
    let Some(last) = path.pop() else {
        return;
    };
    let mut cursor = root;
    for part in path {
        if cursor.get(part).is_none() {
            cursor[part] = json!({});
        }
        cursor = &mut cursor[part];
    }
    cursor[last] = value;
}
