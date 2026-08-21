use super::components::{section_card, toggle_row};
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use crate::core_session::CoreSession;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
) {
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "features-core-hint"
    ));
    input.core_settings.poll();
    if !input.core_settings.started() {
        input.core_settings.start(ai.fetch_core_settings());
    }
    if let Some(Ok(settings)) = &input.core_settings.data
        && let Some(mind) = settings.pointer("/effective/mind")
    {
        draft.seed_core_if_clean("mind", mind.clone());
    }
    ui.add_space(6.0);
    section_card(
        ui,
        "features-mind",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "features-mind"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "features-proactive-hint"
            ));
            let mut enabled = bool_at(draft, "/proactive/enabled");
            if toggle_row(
                ui,
                "proactive-enabled",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "features-proactive-enabled"),
                "",
                &mut enabled,
            ) {
                set_path(draft, "/proactive/enabled", json!(enabled));
            }
            let mut paused = bool_at(draft, "/proactive/paused");
            if toggle_row(
                ui,
                "proactive-paused",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "features-proactive-paused"),
                "",
                &mut paused,
            ) {
                set_path(draft, "/proactive/paused", json!(paused));
            }
            number_u64(
                ui,
                draft,
                "/proactive/cooldown_seconds",
                "features-proactive-cooldown",
            );
            number_u64(
                ui,
                draft,
                "/proactive/min_idle_seconds",
                "features-proactive-min-idle",
            );
            number_u64(
                ui,
                draft,
                "/proactive/observation_interval_seconds",
                "features-proactive-interval",
            );
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
