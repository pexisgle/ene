//! AI & Models page: pick a provider plugin from the host catalog.

use std::path::Path;
use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::{section_card, section_card_collapsible};
use super::draft::SettingsDraft;
use super::gguf_catalog::{self, CatalogEntry, WeightKind};
use super::input::SettingsInputState;
use super::provider_form::{
    ProviderInfo, catalog_from_settings, catalog_plugin, cloud_model_combo, default_cloud_model,
    ids_with_seam, local_plugin, plugin_combo_with_empty, plugin_needs_key, provider_description,
    sidecar_fields,
};
use super::widgets::path_row_filtered;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};

const LEGACY_LOCAL_PLUGIN: &str = "provider.openai_compat";
const GGUF_PLUGIN: &str = "provider.gguf";

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

    let catalog = catalog_from_settings(
        input
            .core_settings
            .data
            .as_ref()
            .and_then(|result| result.as_ref().ok()),
    );

    input.gguf_download.poll();
    input.cloud_models.poll();
    if let Some((id, path, task)) = input.gguf_download.completed_path.take()
        && let Some(entry) = gguf_catalog::entry_by_id(&id)
    {
        let path = path.display().to_string();
        match entry.kind {
            WeightKind::Chat => {
                apply_local(draft, &task, gguf_plugin(&catalog), entry.id, &path);
                entry.id.clone_into(&mut input.local_catalog_id);
            }
            WeightKind::Embedding => {
                apply_local(draft, "embedding", gguf_plugin(&catalog), entry.id, &path);
                entry.id.clone_into(&mut input.embed_catalog_id);
            }
        }
    }

    if input.llama_server_on_path.is_none() {
        input.llama_server_on_path = Some(gguf_catalog::binary_on_path("llama-server"));
    }

    section_card(
        ui,
        "ai-conversation",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-conversation"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-conversation-hint"
            ));
            render_task_plugin(
                ui,
                draft,
                input,
                &catalog,
                "chat",
                &ids_with_seam(&catalog, "seam.llm"),
                None,
            );
            ui.add_space(8.0);
            render_selected_provider(ui, draft, input, ai, &catalog, "chat");
        },
    );

    section_card(
        ui,
        "ai-embedding",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-embedding"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-embed-hint"));
            render_task_plugin(
                ui,
                draft,
                input,
                &catalog,
                "embedding",
                &ids_with_seam(&catalog, "seam.embed"),
                Some("—"),
            );
            ui.add_space(8.0);
            render_selected_provider(ui, draft, input, ai, &catalog, "embedding");
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
            let inherit = i18n_embed_fl::fl!(crate::i18n::loader(), "ai-inherit-chat");
            render_task_plugin(
                ui,
                draft,
                input,
                &catalog,
                "classifier",
                &ids_with_seam(&catalog, "seam.llm"),
                Some(inherit.as_str()),
            );
            ui.add_space(8.0);
            render_selected_provider(ui, draft, input, ai, &catalog, "classifier");
            ui.separator();
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "section-ai-proactive"
            ));
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-advanced-same-as-chat"
            ));
            render_task_plugin(
                ui,
                draft,
                input,
                &catalog,
                "proactive",
                &ids_with_seam(&catalog, "seam.llm"),
                Some(inherit.as_str()),
            );
            ui.add_space(8.0);
            render_selected_provider(ui, draft, input, ai, &catalog, "proactive");
            ui.separator();
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-sidecar-heading"
            ));
            let mut chat = current_binding(draft, "chat");
            if sidecar_fields(ui, &mut chat, &catalog) {
                write_binding(draft, "chat", chat);
            }
        },
    );
}

fn seed_draft_once(draft: &mut SettingsDraft, settings: &Value, input: &mut SettingsInputState) {
    if let Some(ai) = settings.pointer("/effective/ai") {
        let mut ai = ai.clone();
        remap_legacy_local(ai.pointer_mut("/tasks/chat"));
        remap_legacy_local(ai.pointer_mut("/tasks/embedding"));
        remap_legacy_local(ai.pointer_mut("/tasks/classifier"));
        remap_legacy_local(ai.pointer_mut("/tasks/proactive"));
        draft.seed_core_if_clean("ai", ai);
    }
    if input.ai_pickers_seeded {
        return;
    }
    input.ai_api_key.clear();
    input.ai_classifier_key.clear();
    input.ai_embedding_key.clear();
    input.ai_proactive_key.clear();
    input.ai_chat_key_set = flag(settings, "ai_chat_key_set");
    input.ai_classifier_key_set = flag(settings, "ai_classifier_key_set");
    input.ai_embedding_key_set = flag(settings, "ai_embedding_key_set");
    input.ai_proactive_key_set = flag(settings, "ai_proactive_key_set");

    seed_gguf_picker(
        settings.pointer("/effective/ai/tasks/chat"),
        &mut input.local_custom_path,
        &mut input.local_catalog_id,
        WeightKind::Chat,
    );
    seed_gguf_picker(
        settings.pointer("/effective/ai/tasks/classifier"),
        &mut input.local_custom_path,
        &mut input.local_catalog_id,
        WeightKind::Chat,
    );
    seed_gguf_picker(
        settings.pointer("/effective/ai/tasks/proactive"),
        &mut input.local_custom_path,
        &mut input.local_catalog_id,
        WeightKind::Chat,
    );
    seed_gguf_picker(
        settings.pointer("/effective/ai/tasks/embedding"),
        &mut input.embed_custom_path,
        &mut input.embed_catalog_id,
        WeightKind::Embedding,
    );
    input.ai_pickers_seeded = true;
}

fn remap_legacy_local(binding: Option<&mut Value>) {
    let Some(binding) = binding else {
        return;
    };
    let plugin = binding.get("plugin").and_then(Value::as_str).unwrap_or("");
    let has_path = binding
        .get("model_path")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.is_empty());
    if plugin == LEGACY_LOCAL_PLUGIN && has_path {
        binding["plugin"] = json!(GGUF_PLUGIN);
    }
}

fn seed_gguf_picker(
    binding: Option<&Value>,
    custom_path: &mut String,
    catalog_id: &mut String,
    kind: WeightKind,
) {
    let Some(binding) = binding else {
        if catalog_id.is_empty() {
            recommended(kind).id.clone_into(catalog_id);
        }
        return;
    };
    if let Some(path) = binding
        .get("model_path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
    {
        path.clone_into(custom_path);
        let matched = match kind {
            WeightKind::Chat => gguf_catalog::chat_entries()
                .find(|entry| gguf_catalog::catalog_dest(entry).to_string_lossy() == path),
            WeightKind::Embedding => gguf_catalog::embed_entries()
                .find(|entry| gguf_catalog::catalog_dest(entry).to_string_lossy() == path),
        };
        if let Some(entry) = matched {
            entry.id.clone_into(catalog_id);
        }
    }
    if catalog_id.is_empty() {
        recommended(kind).id.clone_into(catalog_id);
    }
}

fn flag(settings: &Value, name: &str) -> bool {
    settings
        .pointer(&format!("/effective/{name}"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn plugin_id(binding: &Value) -> String {
    binding
        .get("plugin")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn gguf_plugin(catalog: &[ProviderInfo]) -> &str {
    local_plugin(catalog, "seam.llm").map_or(GGUF_PLUGIN, |plugin| plugin.id.as_str())
}

fn recommended(kind: WeightKind) -> &'static CatalogEntry {
    match kind {
        WeightKind::Chat => gguf_catalog::recommended_chat(),
        WeightKind::Embedding => gguf_catalog::recommended_embedding(),
    }
}

fn for_each_catalog_entry(kind: WeightKind, mut visit: impl FnMut(&'static CatalogEntry)) {
    match kind {
        WeightKind::Chat => {
            for entry in gguf_catalog::chat_entries() {
                visit(entry);
            }
        }
        WeightKind::Embedding => {
            for entry in gguf_catalog::embed_entries() {
                visit(entry);
            }
        }
    }
}

fn render_task_plugin(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    catalog: &[ProviderInfo],
    task: &str,
    plugins: &[String],
    empty_choice: Option<&str>,
) {
    let previous = plugin_id(&current_binding(draft, task));
    let mut plugin = previous.clone();
    plugin_combo_with_empty(
        ui,
        &format!("ai-{task}-plugin"),
        &mut plugin,
        plugins,
        empty_choice,
    );
    if let Some(desc) = provider_description(&plugin) {
        ui.weak(desc);
    }
    if plugin == previous {
        return;
    }
    if input.core_settings.data.is_none() {
        return;
    }
    if task == "embedding" {
        on_embed_plugin_selected(draft, input, catalog, &plugin);
    } else {
        on_llm_plugin_selected(draft, input, catalog, task, &plugin);
    }
}

fn render_selected_provider(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    ai: &Arc<CoreSession>,
    catalog: &[ProviderInfo],
    task: &str,
) {
    let selected = plugin_id(&current_binding(draft, task));
    match catalog_plugin(catalog, &selected) {
        Some(info) if info.local => {
            let kind = if task == "embedding" {
                WeightKind::Embedding
            } else {
                WeightKind::Chat
            };
            render_gguf(ui, draft, input, ai, task, kind, &info.id);
        }
        Some(info) => render_remote(ui, draft, input, ai, task, &info.id, catalog),
        None => {}
    }
}

fn on_llm_plugin_selected(
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    catalog: &[ProviderInfo],
    task: &str,
    plugin: &str,
) {
    if plugin.is_empty() {
        write_binding(draft, task, json!({ "plugin": "", "model": "" }));
        return;
    }
    let Some(info) = catalog_plugin(catalog, plugin) else {
        return;
    };
    if info.local {
        bind_local_if_ready(draft, input, task, WeightKind::Chat, plugin);
    } else {
        bind_remote(draft, task, plugin);
    }
}

fn on_embed_plugin_selected(
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    catalog: &[ProviderInfo],
    plugin: &str,
) {
    if plugin.is_empty() {
        write_binding(draft, "embedding", json!({ "plugin": "", "model": "" }));
        return;
    }
    let Some(info) = catalog_plugin(catalog, plugin) else {
        return;
    };
    if info.local {
        bind_local_if_ready(draft, input, "embedding", WeightKind::Embedding, plugin);
    } else {
        bind_remote(draft, "embedding", plugin);
    }
}

fn bind_local_if_ready(
    draft: &mut SettingsDraft,
    input: &SettingsInputState,
    task: &str,
    kind: WeightKind,
    plugin: &str,
) {
    let catalog_id = match kind {
        WeightKind::Chat => input.local_catalog_id.as_str(),
        WeightKind::Embedding => input.embed_catalog_id.as_str(),
    };
    let custom_path = match kind {
        WeightKind::Chat => input.local_custom_path.trim(),
        WeightKind::Embedding => input.embed_custom_path.trim(),
    };
    let entry = gguf_catalog::entry_by_id(catalog_id).unwrap_or_else(|| recommended(kind));
    if gguf_catalog::is_downloaded(entry) {
        apply_local(
            draft,
            task,
            plugin,
            entry.id,
            &gguf_catalog::catalog_dest(entry).display().to_string(),
        );
    } else if !custom_path.is_empty() {
        let model = Path::new(custom_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("local-gguf");
        apply_local(draft, task, plugin, model, custom_path);
    } else {
        write_binding(
            draft,
            task,
            json!({
                "plugin": plugin,
                "model": "",
                "model_path": "",
            }),
        );
    }
}

fn bind_remote(draft: &mut SettingsDraft, task: &str, plugin: &str) {
    let mut binding = current_binding(draft, task);
    binding["plugin"] = json!(plugin);
    if let Some(map) = binding.as_object_mut() {
        map.remove("model_path");
    }
    if binding
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        binding["model"] = json!(default_cloud_model(plugin, task));
    }
    write_binding(draft, task, binding);
}

fn render_gguf(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    ai: &Arc<CoreSession>,
    task: &str,
    kind: WeightKind,
    plugin: &str,
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
    let catalog_id = match kind {
        WeightKind::Chat => input.local_catalog_id.as_str(),
        WeightKind::Embedding => input.embed_catalog_id.as_str(),
    };
    let selected = gguf_catalog::entry_by_id(catalog_id).unwrap_or_else(|| recommended(kind));
    let selected_label = catalog_label(selected);
    egui::ComboBox::from_id_salt(format!("ai-gguf-catalog-{task}"))
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            for_each_catalog_entry(kind, |entry| {
                let label = catalog_label(entry);
                let current = match kind {
                    WeightKind::Chat => input.local_catalog_id.as_str(),
                    WeightKind::Embedding => input.embed_catalog_id.as_str(),
                };
                if ui.selectable_label(current == entry.id, label).clicked() {
                    match kind {
                        WeightKind::Chat => entry.id.clone_into(&mut input.local_catalog_id),
                        WeightKind::Embedding => entry.id.clone_into(&mut input.embed_catalog_id),
                    }
                }
            });
        });

    let catalog_id = match kind {
        WeightKind::Chat => input.local_catalog_id.as_str(),
        WeightKind::Embedding => input.embed_catalog_id.as_str(),
    };
    let entry = gguf_catalog::entry_by_id(catalog_id).unwrap_or_else(|| recommended(kind));
    let downloaded = gguf_catalog::is_downloaded(entry);
    ui.horizontal(|ui| {
        if downloaded {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-local-use"))
                .clicked()
            {
                apply_local(
                    draft,
                    task,
                    plugin,
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
                input.gguf_download.start(ai.runtime_handle(), entry, task);
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
    let custom_path = match kind {
        WeightKind::Chat => &mut input.local_custom_path,
        WeightKind::Embedding => &mut input.embed_custom_path,
    };
    if path_row_filtered(
        ui,
        custom_path,
        f32::INFINITY,
        Some((filter_name.as_str(), &["gguf"])),
    ) {
        let path = custom_path.trim();
        if !path.is_empty() {
            let model = Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("local-gguf");
            apply_local(draft, task, plugin, model, path);
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

fn render_remote(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    ai: &Arc<CoreSession>,
    task: &str,
    plugin: &str,
    catalog: &[ProviderInfo],
) {
    let mut binding = current_binding(draft, task);
    binding["plugin"] = json!(plugin);

    if plugin.contains("openai_compat") {
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
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-openai-compat-base-url-hint"
        ));
    }

    render_api_key(ui, draft, input, task, &mut binding, catalog);

    let base_url = binding
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("");
    let key = peek_task_key(input, task).to_owned();
    input.cloud_models.sync(ai, task, plugin, base_url, &key);
    let mut model = binding
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_else(|| default_cloud_model(plugin, task))
        .to_owned();
    if cloud_model_combo(
        ui,
        &format!("ai-cloud-model-{task}-{plugin}"),
        plugin,
        task,
        &mut model,
        input.cloud_models.combo_state(task),
    ) {
        binding["model"] = json!(model);
        write_binding(draft, task, binding.clone());
    }
}

fn apply_local(draft: &mut SettingsDraft, task: &str, plugin: &str, model: &str, model_path: &str) {
    write_binding(
        draft,
        task,
        json!({
            "plugin": plugin,
            "model": model,
            "model_path": model_path,
        }),
    );
}

fn render_api_key(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    task: &str,
    binding: &mut Value,
    catalog: &[ProviderInfo],
) {
    let plugin = binding.get("plugin").and_then(Value::as_str).unwrap_or("");
    if !plugin_needs_key(catalog, plugin) {
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

fn peek_task_key<'a>(input: &'a SettingsInputState, task: &str) -> &'a str {
    match task {
        "classifier" => input.ai_classifier_key.as_str(),
        "embedding" => input.ai_embedding_key.as_str(),
        "proactive" => input.ai_proactive_key.as_str(),
        "tts" => input.ai_tts_key.as_str(),
        "stt" => input.ai_stt_key.as_str(),
        _ => input.ai_api_key.as_str(),
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
