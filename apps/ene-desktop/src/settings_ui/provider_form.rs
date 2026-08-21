//! Provider selector labels for AI / Voice pages.

use serde_json::{Value, json};

pub(crate) const BUILTIN_PROVIDER_I18N_IDS: &[(&str, &str)] = &[
    ("gguf", "gguf"),
    ("openai_compat", "openai-compat"),
    ("anthropic", "anthropic"),
    ("elevenlabs", "elevenlabs"),
    ("voicevox", "voicevox"),
    ("edge_tts", "edge-tts"),
];

/// One row from `effective.providers` (the fiber plugin catalog).
#[derive(Clone, Debug)]
pub struct ProviderInfo {
    pub id: String,
    pub seams: Vec<String>,
    pub local: bool,
    pub needs_key: bool,
}

impl ProviderInfo {
    #[must_use]
    pub fn has_seam(&self, seam: &str) -> bool {
        self.seams.iter().any(|key| key == seam)
    }
}

/// Fallback when core has not published `effective.providers` yet.
/// Keep ids/seams aligned with `ene_fiber::PROVIDER_PLUGINS`.
fn fallback_catalog() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            id: "provider.gguf".to_owned(),
            seams: vec!["seam.llm".to_owned(), "seam.embed".to_owned()],
            local: true,
            needs_key: false,
        },
        ProviderInfo {
            id: "provider.openai_compat".to_owned(),
            seams: vec![
                "seam.llm".to_owned(),
                "seam.embed".to_owned(),
                "seam.tts".to_owned(),
                "seam.stt".to_owned(),
            ],
            local: false,
            needs_key: true,
        },
        ProviderInfo {
            id: "provider.anthropic".to_owned(),
            seams: vec!["seam.llm".to_owned()],
            local: false,
            needs_key: true,
        },
        ProviderInfo {
            id: "provider.elevenlabs".to_owned(),
            seams: vec!["seam.tts".to_owned()],
            local: false,
            needs_key: true,
        },
        ProviderInfo {
            id: "provider.voicevox".to_owned(),
            seams: vec!["seam.tts".to_owned()],
            local: true,
            needs_key: false,
        },
        ProviderInfo {
            id: "provider.edge_tts".to_owned(),
            seams: vec!["seam.tts".to_owned()],
            local: false,
            needs_key: false,
        },
    ]
}

/// Read the host plugin catalog from settings, or the in-tree fallback.
#[must_use]
pub fn catalog_from_settings(settings: Option<&Value>) -> Vec<ProviderInfo> {
    let Some(rows) = settings.and_then(|value| value.pointer("/effective/providers")) else {
        return fallback_catalog();
    };
    let Some(array) = rows.as_array() else {
        return fallback_catalog();
    };
    let parsed: Vec<ProviderInfo> = array.iter().filter_map(parse_provider).collect();
    if parsed.is_empty() {
        fallback_catalog()
    } else {
        parsed
    }
}

fn parse_provider(value: &Value) -> Option<ProviderInfo> {
    let id = value.get("id").and_then(Value::as_str)?.to_owned();
    if !id.starts_with("provider.") {
        return None;
    }
    let seams = value
        .get("seams")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(ProviderInfo {
        id,
        seams,
        local: value.get("local").and_then(Value::as_bool).unwrap_or(false),
        needs_key: value
            .get("needs_key")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[must_use]
pub fn ids_with_seam(catalog: &[ProviderInfo], seam: &str) -> Vec<String> {
    catalog
        .iter()
        .filter(|plugin| plugin.has_seam(seam))
        .map(|plugin| plugin.id.clone())
        .collect()
}

#[must_use]
pub fn local_plugin<'a>(catalog: &'a [ProviderInfo], seam: &str) -> Option<&'a ProviderInfo> {
    catalog
        .iter()
        .find(|plugin| plugin.local && plugin.has_seam(seam))
}

fn provider_i18n_id(kind: &str) -> Option<&'static str> {
    let key = kind.strip_prefix("provider.").unwrap_or(kind);
    BUILTIN_PROVIDER_I18N_IDS
        .iter()
        .find_map(|(provider_kind, i18n_id)| (*provider_kind == key).then_some(*i18n_id))
}

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}

/// Localized provider selector label.
#[must_use]
pub fn provider_display_name(kind: &str) -> String {
    if let Some(i18n_id) = provider_i18n_id(kind) {
        fl(&format!("provider-selector-{i18n_id}-label"))
    } else {
        kind.to_owned()
    }
}

/// Localized provider selector description; `None` for unknown kinds.
#[must_use]
pub fn provider_description(kind: &str) -> Option<String> {
    provider_i18n_id(kind).map(|i18n_id| fl(&format!("provider-selector-{i18n_id}-desc")))
}

/// Localized provider group (Local / Cloud).
#[must_use]
pub fn provider_display_group(kind: &str) -> Option<String> {
    let key = kind.strip_prefix("provider.").unwrap_or(kind);
    match key {
        "gguf" | "voicevox" => Some(fl("provider-selector-group-local")),
        _ => Some(fl("provider-selector-group-cloud")),
    }
}

#[must_use]
pub fn catalog_plugin<'a>(catalog: &'a [ProviderInfo], id: &str) -> Option<&'a ProviderInfo> {
    catalog.iter().find(|plugin| plugin.id == id)
}

#[must_use]
pub fn plugin_needs_key(catalog: &[ProviderInfo], kind: &str) -> bool {
    catalog_plugin(catalog, kind).is_some_and(|plugin| plugin.needs_key)
}

#[must_use]
pub fn plugin_needs_sidecar(catalog: &[ProviderInfo], kind: &str) -> bool {
    catalog_plugin(catalog, kind).is_some_and(|plugin| plugin.local)
}

/// Loopback engine fields (`server_path` / `model_path` / `server_args`).
pub fn sidecar_fields(ui: &mut egui::Ui, binding: &mut Value, catalog: &[ProviderInfo]) -> bool {
    let plugin = binding.get("plugin").and_then(Value::as_str).unwrap_or("");
    if !plugin_needs_sidecar(catalog, plugin) {
        return false;
    }
    let mut changed = false;
    let heading = i18n_embed_fl::fl!(crate::i18n::loader(), "ai-sidecar-heading");
    egui::CollapsingHeader::new(heading)
        .id_salt(("ai-sidecar", plugin))
        .default_open(false)
        .show(ui, |ui| {
            ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-sidecar-hint"));
            changed |= text_field(ui, "ai-server-path-label", binding, "server_path");
            changed |= text_field(ui, "ai-cas-path-label", binding, "cas_path");
            changed |= text_field(ui, "ai-model-path-label", binding, "model_path");
            let mut args = binding
                .get("server_args")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "ai-server-args-label"
            ));
            if ui
                .add(egui::TextEdit::singleline(&mut args).desired_width(f32::INFINITY))
                .changed()
            {
                binding["server_args"] = json!(
                    args.split_whitespace()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                );
                changed = true;
            }
        });
    changed
}

fn text_field(ui: &mut egui::Ui, label: &str, binding: &mut Value, key: &str) -> bool {
    let mut value = binding
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    ui.label(fl(label));
    if ui
        .add(egui::TextEdit::singleline(&mut value).desired_width(f32::INFINITY))
        .changed()
    {
        binding[key] = json!(value);
        true
    } else {
        false
    }
}

const OPENAI_CHAT_MODELS: &[&str] = &[
    "gpt-4.1-mini",
    "gpt-4.1",
    "gpt-4o-mini",
    "gpt-4o",
    "o4-mini",
];

const OPENAI_EMBED_MODELS: &[&str] = &["text-embedding-3-small", "text-embedding-3-large"];

const OPENAI_TTS_MODELS: &[&str] = &["gpt-4o-mini-tts", "gpt-4o-tts", "tts-1", "tts-1-hd"];

const OPENAI_STT_MODELS: &[&str] = &["whisper-1", "gpt-4o-mini-transcribe", "gpt-4o-transcribe"];

const CLAUDE_MODELS: &[&str] = &[
    "claude-sonnet-4-5",
    "claude-opus-4-5",
    "claude-haiku-4-5",
    "claude-sonnet-4-20250514",
    "claude-opus-4-20250514",
    "claude-haiku-4-5-20251001",
];

fn plugin_kind(plugin: &str) -> &str {
    plugin.strip_prefix("provider.").unwrap_or(plugin)
}

/// Known model ids for a catalog plugin + task. Empty for plugins without a list.
#[must_use]
pub fn cloud_model_presets(plugin: &str, task: &str) -> &'static [&'static str] {
    match (plugin_kind(plugin), task) {
        ("anthropic", _) => CLAUDE_MODELS,
        ("openai_compat", "embedding") => OPENAI_EMBED_MODELS,
        ("openai_compat", "tts") => OPENAI_TTS_MODELS,
        ("openai_compat", "stt") => OPENAI_STT_MODELS,
        ("openai_compat", _) => OPENAI_CHAT_MODELS,
        _ => &[],
    }
}

#[must_use]
pub fn default_cloud_model(plugin: &str, task: &str) -> &'static str {
    cloud_model_presets(plugin, task)
        .first()
        .copied()
        .unwrap_or("")
}

/// Extra rows for [`cloud_model_combo`]: live `/models` ids, if any.
#[derive(Clone, Copy, Debug)]
pub struct CloudModelComboState<'a> {
    pub live: &'a [String],
    pub loading: bool,
    pub error: Option<&'a str>,
}

/// Combo of known cloud models, matching the plugin picker.
///
/// OpenAI-compatible hosts also get a free-form id for other `/v1` endpoints.
/// Returns whether `model` changed.
pub fn cloud_model_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    plugin: &str,
    task: &str,
    model: &mut String,
    remote: CloudModelComboState<'_>,
) -> bool {
    let presets = cloud_model_presets(plugin, task);
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-model-label"));
    if presets.is_empty() && remote.live.is_empty() {
        return ui
            .add(egui::TextEdit::singleline(model).desired_width(f32::INFINITY))
            .changed();
    }

    let fallback: Vec<String> = presets.iter().map(|name| (*name).to_owned()).collect();
    let rows = if remote.live.is_empty() {
        fallback.as_slice()
    } else {
        remote.live
    };

    let mut changed = false;
    let selected = if model.is_empty() {
        "—"
    } else {
        model.as_str()
    };
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected.to_owned())
        .show_ui(ui, |ui| {
            for id in rows {
                if ui.selectable_label(model.as_str() == id, id).clicked() && model.as_str() != id {
                    id.clone_into(model);
                    changed = true;
                }
            }
        });
    if remote.loading {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-model-loading"
        ));
    }
    if let Some(error) = remote.error {
        ui.colored_label(
            egui::Color32::from_rgb(0xff, 0x8a, 0x65),
            i18n_embed_fl::fl!(crate::i18n::loader(), "ai-model-list-error", error = error),
        );
    }

    if plugin_kind(plugin) == "openai_compat" {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-model-custom-hint"
        ));
        changed |= ui
            .add(egui::TextEdit::singleline(model).desired_width(f32::INFINITY))
            .changed();
    }
    changed
}

/// Combo box of bundled provider ids, grouped Local / Cloud.
pub fn plugin_combo(ui: &mut egui::Ui, id_salt: &str, plugin: &mut String, plugins: &[String]) {
    plugin_combo_with_empty(ui, id_salt, plugin, plugins, None);
}

/// `empty_choice` is the first row that clears the plugin id (`None` hides it).
pub fn plugin_combo_with_empty(
    ui: &mut egui::Ui,
    id_salt: &str,
    plugin: &mut String,
    plugins: &[String],
    empty_choice: Option<&str>,
) {
    let selected = if plugin.is_empty() {
        empty_choice.unwrap_or("—").to_owned()
    } else {
        provider_display_name(plugin)
    };
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if let Some(label) = empty_choice
                && ui.selectable_label(plugin.is_empty(), label).clicked()
            {
                plugin.clear();
            }
            let mut last_group = String::new();
            for candidate in plugins {
                let group = provider_display_group(candidate).unwrap_or_default();
                if group != last_group {
                    ui.weak(&group);
                    last_group.clone_from(&group);
                }
                if ui
                    .selectable_label(
                        plugin.as_str() == candidate,
                        provider_display_name(candidate),
                    )
                    .clicked()
                {
                    candidate.clone_into(plugin);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{cloud_model_presets, default_cloud_model};

    #[test]
    fn openai_and_anthropic_expose_selectable_lists() {
        assert!(cloud_model_presets("provider.openai_compat", "chat").contains(&"gpt-4.1-mini"));
        assert!(
            cloud_model_presets("provider.openai_compat", "embedding")
                .contains(&"text-embedding-3-small")
        );
        assert!(cloud_model_presets("provider.anthropic", "chat").contains(&"claude-sonnet-4-5"));
        assert_eq!(
            default_cloud_model("provider.openai_compat", "chat"),
            "gpt-4.1-mini"
        );
        assert_eq!(
            default_cloud_model("provider.anthropic", "classifier"),
            "claude-sonnet-4-5"
        );
        assert!(cloud_model_presets("provider.gguf", "chat").is_empty());
    }
}
