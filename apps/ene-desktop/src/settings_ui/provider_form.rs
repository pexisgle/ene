//! Provider selector labels for AI / Voice pages.

pub(crate) const BUILTIN_PROVIDER_I18N_IDS: &[(&str, &str)] = &[
    ("echo", "echo"),
    ("openai_compat", "openai-compat"),
    ("anthropic", "anthropic"),
    ("elevenlabs", "elevenlabs"),
    ("voicevox", "voicevox"),
    ("edge_tts", "edge-tts"),
];

pub(crate) const CHAT_PLUGINS: &[&str] = &["echo", "provider.openai_compat", "provider.anthropic"];

pub(crate) const EMBED_PLUGINS: &[&str] = &["echo", "provider.openai_compat"];

pub(crate) const AUDIO_PLUGINS: &[&str] = &[
    "echo",
    "provider.voicevox",
    "provider.openai_compat",
    "provider.elevenlabs",
    "provider.edge_tts",
];

pub(crate) const STT_PLUGINS: &[&str] = &["echo", "provider.openai_compat"];

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

/// Localized provider group (Host / Local / Cloud).
#[must_use]
pub fn provider_display_group(kind: &str) -> Option<String> {
    let key = kind.strip_prefix("provider.").unwrap_or(kind);
    match key {
        "echo" => Some(fl("provider-selector-group-host")),
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

/// Combo box of bundled provider ids, grouped Host / Local / Cloud.
pub fn plugin_combo(ui: &mut egui::Ui, id_salt: &str, plugin: &mut String, plugins: &[&str]) {
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(provider_display_name(plugin))
        .show_ui(ui, |ui| {
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
