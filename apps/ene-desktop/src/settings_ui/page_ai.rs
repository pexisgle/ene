//! AI & Models page: pick This computer / `ChatGPT` / Claude.

use std::path::Path;
use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::{section_card, section_card_collapsible};
use super::draft::SettingsDraft;
use super::gguf_catalog::{self, CatalogEntry};
use super::input::{ChatMode, SettingsInputState};
use super::provider_form::{
    CHAT_PLUGINS, EMBED_PLUGINS, plugin_combo_with_empty, plugin_needs_key, provider_description,
    sidecar_fields,
};
use super::widgets::{editable_combo, path_row_filtered};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};

const OPENAI_MODELS: &[&str] = &[
    "gpt-4o-mini",
    "gpt-4o",
    "gpt-4.1-mini",
    "gpt-4.1",
    "o4-mini",
];

const CLAUDE_MODELS: &[&str] = &[
    "claude-sonnet-4-20250514",
    "claude-opus-4-20250514",
    "claude-haiku-4-5-20251001",
];

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

    input.gguf_download.poll();
    if let Some((id, path)) = input.gguf_download.completed_path.take() {
        apply_local_binding(draft, &id, &path.display().to_string());
        input.chat_mode = Some(ChatMode::Local);
        input.local_catalog_id.clone_from(&id);
    }

    if input.llama_server_on_path.is_none() {
        input.llama_server_on_path = Some(gguf_catalog::binary_on_path("llama-server"));
    }

    let binding = current_binding(draft, "chat");
    let mode = input.chat_mode.unwrap_or_else(|| detect_mode(&binding));

    section_card(
        ui,
        "ai-conversation",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-conversation"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-conversation-hint"
            ));
            ui.horizontal(|ui| {
                let modes = [
                    (
                        ChatMode::Local,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "ai-mode-local"),
                    ),
                    (
                        ChatMode::OpenAiCompat,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "ai-mode-openai"),
                    ),
                    (
                        ChatMode::Claude,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "ai-mode-claude"),
                    ),
                ];
                for (candidate, label) in &modes {
                    if ui.selectable_label(mode == *candidate, label).clicked() {
                        input.chat_mode = Some(*candidate);
                        on_mode_selected(draft, input, *candidate);
                    }
                }
            });
            ui.add_space(8.0);
            match mode {
                ChatMode::Local => render_local(ui, draft, input, ai),
                ChatMode::OpenAiCompat => render_openai(ui, draft, input),
                ChatMode::Claude => render_claude(ui, draft, input),
            }
        },
    );

    section_card_collapsible(
        ui,
        "ai-advanced",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-advanced"),
        false,
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-advanced-hint"
            ));
            ui.separator();
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "section-ai-classifier"
            ));
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-advanced-same-as-chat"
            ));
            render_binding(ui, draft, input, "classifier", CHAT_PLUGINS);
            ui.separator();
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "section-ai-proactive"
            ));
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-advanced-same-as-chat"
            ));
            render_binding(ui, draft, input, "proactive", CHAT_PLUGINS);
            ui.separator();
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "section-ai-embedding"
            ));
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-advanced-embedding-unused"
            ));
            render_binding(ui, draft, input, "embedding", EMBED_PLUGINS);
            ui.separator();
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-sidecar-heading"
            ));
            let mut chat = current_binding(draft, "chat");
            if sidecar_fields(ui, &mut chat) {
                write_binding(draft, "chat", chat);
            }
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

    let chat = settings
        .pointer("/effective/ai/tasks/chat")
        .cloned()
        .unwrap_or_else(|| json!({}));
    input.chat_mode = Some(detect_mode(&chat));
    if let Some(path) = chat.get("model_path").and_then(Value::as_str) {
        path.clone_into(&mut input.local_custom_path);
        if let Some(entry) = gguf_catalog::CATALOG
            .iter()
            .find(|entry| gguf_catalog::catalog_dest(entry).to_string_lossy() == path)
        {
            entry.id.clone_into(&mut input.local_catalog_id);
        }
    }
    if input.local_catalog_id.is_empty() {
        gguf_catalog::recommended_entry()
            .id
            .clone_into(&mut input.local_catalog_id);
    }
}

fn flag(settings: &Value, name: &str) -> bool {
    settings
        .pointer(&format!("/effective/{name}"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn detect_mode(binding: &Value) -> ChatMode {
    let plugin = binding.get("plugin").and_then(Value::as_str).unwrap_or("");
    let model_path = binding
        .get("model_path")
        .and_then(Value::as_str)
        .unwrap_or("");
    match plugin {
        "provider.anthropic" => ChatMode::Claude,
        "provider.openai_compat" if !model_path.is_empty() => ChatMode::Local,
        "provider.openai_compat" => ChatMode::OpenAiCompat,
        _ => ChatMode::Local,
    }
}

fn on_mode_selected(draft: &mut SettingsDraft, input: &mut SettingsInputState, mode: ChatMode) {
    match mode {
        ChatMode::Local => {
            let entry = gguf_catalog::entry_by_id(&input.local_catalog_id)
                .unwrap_or_else(gguf_catalog::recommended_entry);
            if gguf_catalog::is_downloaded(entry) {
                apply_local_binding(
                    draft,
                    entry.id,
                    &gguf_catalog::catalog_dest(entry).display().to_string(),
                );
            } else if !input.local_custom_path.trim().is_empty() {
                let path = input.local_custom_path.trim();
                let model = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("local-gguf");
                apply_local_binding(draft, model, path);
            }
        }
        ChatMode::OpenAiCompat => {
            let mut binding = current_binding(draft, "chat");
            binding["plugin"] = json!("provider.openai_compat");
            if let Some(map) = binding.as_object_mut() {
                map.remove("model_path");
            }
            if binding
                .get("model")
                .and_then(Value::as_str)
                .is_none_or(|m| m.is_empty() || m.starts_with("gemma-"))
            {
                binding["model"] = json!(OPENAI_MODELS[0]);
            }
            write_binding(draft, "chat", binding);
        }
        ChatMode::Claude => {
            let mut binding = json!({
                "plugin": "provider.anthropic",
                "model": CLAUDE_MODELS[0],
            });
            let prev = current_binding(draft, "chat");
            if let Some(model) = prev.get("model").and_then(Value::as_str)
                && model.starts_with("claude-")
            {
                binding["model"] = json!(model);
            }
            write_binding(draft, "chat", binding);
        }
    }
}

fn render_local(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    ai: &Arc<CoreSession>,
) {
    if input.llama_server_on_path != Some(true) {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-llama-server-missing"
        ));
    }

    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "ai-local-catalog-label"
    ));
    let selected = gguf_catalog::entry_by_id(&input.local_catalog_id)
        .unwrap_or_else(gguf_catalog::recommended_entry);
    let selected_label = catalog_label(selected);
    egui::ComboBox::from_id_salt("ai-local-catalog")
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            for entry in gguf_catalog::CATALOG {
                let label = catalog_label(entry);
                if ui
                    .selectable_label(input.local_catalog_id == entry.id, label)
                    .clicked()
                {
                    entry.id.clone_into(&mut input.local_catalog_id);
                }
            }
        });

    let entry = gguf_catalog::entry_by_id(&input.local_catalog_id)
        .unwrap_or_else(gguf_catalog::recommended_entry);
    let downloaded = gguf_catalog::is_downloaded(entry);
    ui.horizontal(|ui| {
        if downloaded {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-local-use"))
                .clicked()
            {
                apply_local_binding(
                    draft,
                    entry.id,
                    &gguf_catalog::catalog_dest(entry).display().to_string(),
                );
            }
        } else {
            let busy =
                input.gguf_download.busy() && input.gguf_download.entry_id() == Some(entry.id);
            let label = if busy {
                i18n_embed_fl::fl!(crate::i18n::loader(), "ai-local-downloading")
            } else {
                i18n_embed_fl::fl!(crate::i18n::loader(), "ai-local-download")
            };
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                input.gguf_download.start(ai.runtime_handle(), entry);
            }
        }
    });

    if input.gguf_download.busy() && input.gguf_download.entry_id() == Some(entry.id) {
        let snap = input.gguf_download.progress_snapshot();
        let fraction = snap
            .total
            .filter(|total| *total > 0)
            .map_or(0.0, |total| snap.received as f32 / total as f32);
        ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
        if let Some(total) = snap.total {
            ui.weak(format!(
                "{} / {}",
                format_bytes(snap.received),
                format_bytes(total)
            ));
        } else {
            ui.weak(format_bytes(snap.received));
        }
    }
    if let Some(err) = &input.gguf_download.last_error {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x8a, 0x65), err);
    }

    ui.add_space(6.0);
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "ai-local-custom-label"
    ));
    let filter_name = i18n_embed_fl::fl!(crate::i18n::loader(), "ai-gguf-filter");
    if path_row_filtered(
        ui,
        &mut input.local_custom_path,
        f32::INFINITY,
        Some((filter_name.as_str(), &["gguf"])),
    ) {
        let path = input.local_custom_path.trim();
        if !path.is_empty() {
            let model = Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("local-gguf");
            apply_local_binding(draft, model, path);
        }
    }
}

fn catalog_label(entry: &CatalogEntry) -> String {
    if entry.recommended {
        i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-catalog-recommended",
            name = entry.id
        )
    } else {
        i18n_embed_fl::fl!(crate::i18n::loader(), "ai-catalog-smarter", name = entry.id)
    }
}

fn format_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let n = n as f64;
    if n >= GIB {
        format!("{:.2} GiB", n / GIB)
    } else if n >= MIB {
        format!("{:.1} MiB", n / MIB)
    } else if n >= KIB {
        format!("{:.0} KiB", n / KIB)
    } else {
        format!("{n:.0} B")
    }
}

fn render_openai(ui: &mut egui::Ui, draft: &mut SettingsDraft, input: &mut SettingsInputState) {
    let mut binding = current_binding(draft, "chat");
    binding["plugin"] = json!("provider.openai_compat");
    if let Some(map) = binding.as_object_mut() {
        map.remove("model_path");
    }

    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-model-label"));
    let mut model = binding
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(OPENAI_MODELS[0])
        .to_owned();
    let choices: Vec<(String, String)> = OPENAI_MODELS
        .iter()
        .map(|m| ((*m).to_owned(), (*m).to_owned()))
        .collect();
    let combo = editable_combo(ui, "ai-openai-model", &mut model, &choices, f32::INFINITY);
    if combo.commit_requested() {
        binding["model"] = json!(model);
        write_binding(draft, "chat", binding.clone());
    }

    render_api_key(ui, draft, input, "chat", &mut binding);

    egui::CollapsingHeader::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "ai-openai-advanced"
    ))
    .id_salt("ai-openai-advanced")
    .default_open(false)
    .show(ui, |ui| {
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
            write_binding(draft, "chat", binding.clone());
        }
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-openai-compat-base-url-hint"
        ));
    });
}

fn render_claude(ui: &mut egui::Ui, draft: &mut SettingsDraft, input: &mut SettingsInputState) {
    let mut binding = current_binding(draft, "chat");
    binding["plugin"] = json!("provider.anthropic");
    if let Some(map) = binding.as_object_mut() {
        map.remove("model_path");
        map.remove("base_url");
    }

    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-model-label"));
    let mut model = binding
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(CLAUDE_MODELS[0])
        .to_owned();
    let choices: Vec<(String, String)> = CLAUDE_MODELS
        .iter()
        .map(|m| ((*m).to_owned(), (*m).to_owned()))
        .collect();
    let combo = editable_combo(ui, "ai-claude-model", &mut model, &choices, f32::INFINITY);
    if combo.commit_requested() {
        binding["model"] = json!(model);
        write_binding(draft, "chat", binding.clone());
    }

    render_api_key(ui, draft, input, "chat", &mut binding);
}

fn render_api_key(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    task: &str,
    binding: &mut Value,
) {
    let plugin = binding.get("plugin").and_then(Value::as_str).unwrap_or("");
    if !plugin_needs_key(plugin) {
        return;
    }
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
        binding["api_key"] = json!(buffer.clone());
        write_binding(draft, task, binding.clone());
    }
}

fn apply_local_binding(draft: &mut SettingsDraft, model: &str, model_path: &str) {
    write_binding(
        draft,
        "chat",
        json!({
            "plugin": "provider.openai_compat",
            "model": model,
            "model_path": model_path,
        }),
    );
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
        .unwrap_or("")
        .to_owned();
    plugin_combo_with_empty(ui, &format!("ai-{task}-plugin"), &mut plugin, plugins, true);
    if let Some(desc) = provider_description(&plugin) {
        ui.weak(desc);
    }
    if plugin != binding.get("plugin").and_then(Value::as_str).unwrap_or("") {
        binding["plugin"] = json!(plugin.clone());
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

    render_api_key(ui, draft, input, task, &mut binding);
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
        .unwrap_or_else(|| json!({ "plugin": "", "model": "" }))
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
