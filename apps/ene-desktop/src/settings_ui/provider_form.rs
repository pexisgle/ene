//! Provider selector labels for AI / Voice pages.

use serde_json::{Value, json};

pub(crate) const BUILTIN_PROVIDER_I18N_IDS: &[(&str, &str)] = &[
    ("openai_compat", "openai-compat"),
    ("anthropic", "anthropic"),
    ("elevenlabs", "elevenlabs"),
    ("voicevox", "voicevox"),
    ("edge_tts", "edge-tts"),
];

pub(crate) const CHAT_PLUGINS: &[&str] = &["provider.openai_compat", "provider.anthropic"];

pub(crate) const EMBED_PLUGINS: &[&str] = &["provider.openai_compat"];

pub(crate) const AUDIO_PLUGINS: &[&str] = &[
    "provider.voicevox",
    "provider.openai_compat",
    "provider.elevenlabs",
    "provider.edge_tts",
];

pub(crate) const STT_PLUGINS: &[&str] = &["provider.openai_compat"];

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
        "voicevox" => Some(fl("provider-selector-group-local")),
        _ => Some(fl("provider-selector-group-cloud")),
    }
}

#[must_use]
pub fn plugin_needs_key(kind: &str) -> bool {
    matches!(
        kind.strip_prefix("provider.").unwrap_or(kind),
        "openai_compat" | "anthropic" | "elevenlabs"
    )
}

#[must_use]
pub fn plugin_needs_sidecar(kind: &str) -> bool {
    matches!(
        kind.strip_prefix("provider.").unwrap_or(kind),
        "openai_compat" | "voicevox"
    )
}

/// Loopback engine fields (`server_path` / `model_path` / `server_args`).
pub fn sidecar_fields(ui: &mut egui::Ui, binding: &mut Value) -> bool {
    let plugin = binding.get("plugin").and_then(Value::as_str).unwrap_or("");
    if !plugin_needs_sidecar(plugin) {
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

/// Combo box of bundled provider ids, grouped Local / Cloud.
///
/// When `allow_empty` is true, the first choice clears the plugin id so
/// advanced tasks can stay unset (same as chat / unused).
pub fn plugin_combo(ui: &mut egui::Ui, id_salt: &str, plugin: &mut String, plugins: &[&str]) {
    plugin_combo_with_empty(ui, id_salt, plugin, plugins, false);
}

pub fn plugin_combo_with_empty(
    ui: &mut egui::Ui,
    id_salt: &str,
    plugin: &mut String,
    plugins: &[&str],
    allow_empty: bool,
) {
    let selected = if plugin.is_empty() && allow_empty {
        "—".to_owned()
    } else {
        provider_display_name(plugin)
    };
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if allow_empty && ui.selectable_label(plugin.is_empty(), "—").clicked() {
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
                        plugin.as_str() == *candidate,
                        provider_display_name(candidate),
                    )
                    .clicked()
                {
                    (*candidate).clone_into(plugin);
                }
            }
        });
}
