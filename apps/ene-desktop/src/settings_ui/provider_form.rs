//! Provider selector labels for Voice / Engines pages.
//!
//! Provider config itself is edited as core JSON (`plugins` / `ai` sections)
//! rather than through an in-process plugin-host snapshot.
#![expect(
    dead_code,
    reason = "provider form metadata stays so i18n tests can keep covering built-in ids"
)]

pub(crate) const BUILTIN_PROVIDER_I18N_IDS: &[(&str, &str)] = &[
    ("kokoro", "kokoro"),
    ("voicevox", "voicevox"),
    ("whisper", "whisper"),
    ("edge-tts", "edge-tts"),
    ("openai_tts", "openai-tts"),
    ("elevenlabs", "elevenlabs"),
];

fn provider_i18n_id(kind: &str) -> Option<&'static str> {
    BUILTIN_PROVIDER_I18N_IDS
        .iter()
        .find_map(|(provider_kind, i18n_id)| (*provider_kind == kind).then_some(*i18n_id))
}

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}

/// Localized provider selector label (Voice page combo).
#[must_use]
pub fn provider_display_name(kind: &str) -> String {
    if let Some(i18n_id) = provider_i18n_id(kind) {
        fl(&format!("provider-selector-{i18n_id}-label"))
    } else {
        kind.to_string()
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
    const LOCAL: &[&str] = &["kokoro", "voicevox", "whisper"];
    const CLOUD: &[&str] = &["edge-tts", "openai_tts", "elevenlabs"];
    if LOCAL.contains(&kind) {
        Some(fl("provider-selector-group-local"))
    } else if CLOUD.contains(&kind) {
        Some(fl("provider-selector-group-cloud"))
    } else {
        None
    }
}

/// Maps a provider kind to the plugin list name owning its config.
#[must_use]
pub fn plugin_name_for_provider_kind(kind: &str) -> String {
    if kind == "openai_tts" {
        "openai-tts".to_string()
    } else {
        kind.to_string()
    }
}
