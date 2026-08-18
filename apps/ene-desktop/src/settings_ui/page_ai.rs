use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::section_card;
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use super::provider_form::{
    CHAT_PLUGINS, EMBED_PLUGINS, plugin_combo, plugin_needs_key, provider_description,
    sidecar_fields,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};

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
    if let Some(Ok(settings)) = input.core_settings.data.clone() {
        seed_draft_once(draft, &settings, input);
    }

    section_card(
        ui,
        "ai-chat",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-chat"),
        |ui| {
            render_binding(ui, draft, input, "chat", CHAT_PLUGINS);
        },
    );
    section_card(
        ui,
        "ai-classifier",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-classifier"),
        |ui| {
            render_binding(ui, draft, input, "classifier", CHAT_PLUGINS);
        },
    );
    section_card(
        ui,
        "ai-proactive",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-proactive"),
        |ui| {
            render_binding(ui, draft, input, "proactive", CHAT_PLUGINS);
        },
    );
    section_card(
        ui,
        "ai-embedding",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-embedding"),
        |ui| {
            render_binding(ui, draft, input, "embedding", EMBED_PLUGINS);
        },
    );
}

fn seed_draft_once(draft: &mut SettingsDraft, settings: &Value, input: &mut SettingsInputState) {
    if draft.editing().section_value("ai").is_some() {
        return;
    }
    if let Some(ai) = settings.pointer("/effective/ai") {
        draft.seed_core_section("ai", ai.clone());
    }
    input.ai_api_key.clear();
    input.ai_classifier_key.clear();
    input.ai_embedding_key.clear();
    input.ai_proactive_key.clear();
    input.ai_chat_key_set = flag(settings, "ai_chat_key_set");
    input.ai_classifier_key_set = flag(settings, "ai_classifier_key_set");
    input.ai_embedding_key_set = flag(settings, "ai_embedding_key_set");
    input.ai_proactive_key_set = flag(settings, "ai_proactive_key_set");
}

fn flag(settings: &Value, name: &str) -> bool {
    settings
        .pointer(&format!("/effective/{name}"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn render_binding(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    task: &str,
    plugins: &[&str],
) {
    let mut binding = current_binding(draft, task);
    let mut plugin = binding
        .get("plugin")
        .and_then(Value::as_str)
        .unwrap_or("echo")
        .to_owned();
    plugin_combo(ui, &format!("ai-{task}-plugin"), &mut plugin, plugins);
    if let Some(desc) = provider_description(&plugin) {
        ui.weak(desc);
    }
    if plugin != binding.get("plugin").and_then(Value::as_str).unwrap_or("") {
        binding["plugin"] = json!(plugin.clone());
        if plugin == "echo" {
            binding["model"] = json!("echo");
        }
        write_binding(draft, task, binding.clone());
    }

    let mut model = binding
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-model-label"));
    if ui
        .add(egui::TextEdit::singleline(&mut model).desired_width(f32::INFINITY))
        .changed()
    {
        binding["model"] = json!(model);
        write_binding(draft, task, binding.clone());
    }

    let mut base_url = binding
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "ai-base-url-label"
    ));
    if ui
        .add(egui::TextEdit::singleline(&mut base_url).desired_width(f32::INFINITY))
        .changed()
    {
        binding["base_url"] = json!(base_url);
        write_binding(draft, task, binding.clone());
    }

    let mut max_tokens = binding
        .get("max_tokens")
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .unwrap_or_default();
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "ai-max-tokens-label"
    ));
    if ui
        .add(egui::TextEdit::singleline(&mut max_tokens).desired_width(120.0))
        .changed()
    {
        if max_tokens.trim().is_empty() {
            if let Some(map) = binding.as_object_mut() {
                map.remove("max_tokens");
            }
        } else if let Ok(n) = max_tokens.trim().parse::<u32>() {
            binding["max_tokens"] = json!(n);
        }
        write_binding(draft, task, binding.clone());
    }

    if sidecar_fields(ui, &mut binding) {
        write_binding(draft, task, binding.clone());
    }

    if plugin_needs_key(&plugin) {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-api-key-label"
        ));
        let (buffer, set) = task_key_buffer(input, task);
        if set && buffer.is_empty() {
            ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-key-set"));
        }
        if ui
            .add(
                egui::TextEdit::singleline(buffer)
                    .password(true)
                    .desired_width(f32::INFINITY),
            )
            .changed()
            && !buffer.is_empty()
        {
            let mut next = current_binding(draft, task);
            next["api_key"] = json!(buffer.clone());
            write_binding(draft, task, next);
        }
    }
}

fn task_key_buffer<'a>(input: &'a mut SettingsInputState, task: &str) -> (&'a mut String, bool) {
    match task {
        "classifier" => (&mut input.ai_classifier_key, input.ai_classifier_key_set),
        "embedding" => (&mut input.ai_embedding_key, input.ai_embedding_key_set),
        "proactive" => (&mut input.ai_proactive_key, input.ai_proactive_key_set),
        _ => (&mut input.ai_api_key, input.ai_chat_key_set),
    }
}

fn current_binding(draft: &SettingsDraft, task: &str) -> Value {
    draft
        .editing()
        .section_value("ai")
        .and_then(|ai| ai.pointer(&format!("/tasks/{task}")).cloned())
        .unwrap_or_else(|| json!({ "plugin": "echo", "model": "echo" }))
}

fn write_binding(draft: &mut SettingsDraft, task: &str, binding: Value) {
    let mut ai = draft
        .editing()
        .section_value("ai")
        .unwrap_or_else(|| json!({ "tasks": {} }));
    if !ai.get("tasks").is_some_and(Value::is_object) {
        ai["tasks"] = json!({});
    }
    ai["tasks"][task] = binding;
    draft.set_section_value("ai", ai);
}
