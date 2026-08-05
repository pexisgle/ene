//! Provider-specific settings sourced from the `plugins.list.<name>` sections.
//!
//! These settings used to live in `AiConfig` (which `ene-ai` never read
//! itself) and were consumed only by the in-process local providers
//! (the local-llm plugin's llama.cpp loader, `ene-voice`'s ONNX engines).
//! They moved to host-opaque plugin config blobs in preparation for the
//! provider-plugin epic; until the destination plugins exist, the in-process
//! readers fall back to reading them from here.
//!
//! # Where the document comes from
//!
//! Readers fall into two groups. The plugin-host adapters receive the full
//! `EneConfig` at construction and read their plugin settings from that
//! document via [`plugin_config_blob`] / [`plugin_profile_blob`] — e.g. the
//! host's `IpcTtsProvider` / `IpcSttProvider` / `IpcVadEngine` re-read the
//! blob on every call so `plugins.list.<name>.config` edits apply live.
//! Paths that only hold the `AiConfig` section (or none at all) fall back to
//! the global config singleton ([`ene_config::get_global_config`]) via
//! [`global_plugin_config_blob`] / [`global_plugin_profile_blob`] /
//! [`LlamaCppPluginConfig::global`] — e.g. the local-llm plugin's llama.cpp
//! loader and the resolve-time reads in [`crate::resolve::ResolvedLocalModel`].
//!
//! The global singleton is refreshed on every `load_full_config_from` /
//! `ConfigStore` load, so a settings edit made after startup is picked up by
//! the global-singleton readers only after the next full config load. That
//! staleness is acceptable for these niche loader settings; a later epic
//! should thread the active `EneConfig` into the remaining singleton readers.

use ene_config::EneConfig;

use crate::config::ProactiveAcceleration;
use ene_config::schemars;

/// Plugin list key for the llama.cpp local-inference plugin.
pub const LLAMA_CPP_PLUGIN: &str = "llama-cpp";
/// Plugin list key for the ONNX (TTS/VAD) plugin.
pub const ONNX_PLUGIN: &str = "onnx";
/// Plugin list key for the Kokoro TTS plugin.
pub const KOKORO_PLUGIN: &str = "kokoro";
/// Plugin list key for the whisper.cpp STT plugin.
pub const WHISPER_PLUGIN: &str = "whisper";
/// Default profile name under `plugins.list.kokoro.profiles` for the single
/// Kokoro voice set shipped today.
pub const KOKORO_DEFAULT_PROFILE: &str = "kokoro";

/// Reads an opaque `plugins.list.<name>.config` blob from a config document.
///
/// Returns `None` when the blob is absent or `null`.
#[must_use]
pub fn plugin_config_blob(config: &EneConfig, name: &str) -> Option<serde_json::Value> {
    config
        .get_path(&format!("plugins.list.{name}.config"))
        .filter(|v| !v.is_null())
}

/// Reads an opaque `plugins.list.<name>.profiles.<profile>` blob from a
/// config document. Returns `None` when the profile is absent or `null`.
#[must_use]
pub fn plugin_profile_blob(
    config: &EneConfig,
    name: &str,
    profile: &str,
) -> Option<serde_json::Value> {
    config
        .get_path(&format!("plugins.list.{name}.profiles.{profile}"))
        .filter(|v| !v.is_null())
}

/// Reads a `plugins.list.<name>.config` blob from the global config
/// singleton. See the module docs for the staleness caveat.
#[must_use]
pub fn global_plugin_config_blob(name: &str) -> Option<serde_json::Value> {
    plugin_config_blob(&ene_config::get_global_config(), name)
}

/// Reads a `plugins.list.<name>.profiles.<profile>` blob from the global
/// config singleton. See the module docs for the staleness caveat.
#[must_use]
pub fn global_plugin_profile_blob(name: &str, profile: &str) -> Option<serde_json::Value> {
    plugin_profile_blob(&ene_config::get_global_config(), name, profile)
}

/// llama.cpp plugin settings (`plugins.list.llama-cpp.config`).
///
/// Holds the multimodal-projector (`mmproj`) settings and the preferred
/// acceleration backend that previously lived per-model on
/// [`LocalModelDef`](crate::config::LocalModelDef) but are only read by the
/// llama.cpp loader. Until the llama.cpp provider plugin exists, these are
/// global (not per-model) plugin-level settings.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct LlamaCppPluginConfig {
    /// HTTPS URL for the multimodal projector (`mmproj`) GGUF.
    #[serde(default)]
    pub mmproj_url: String,
    /// Optional filesystem path for `mmproj` (skips download when non-empty).
    #[serde(default)]
    pub mmproj_path: String,
    /// Preferred acceleration backend.
    #[serde(default)]
    pub acceleration: ProactiveAcceleration,
}

impl Default for LlamaCppPluginConfig {
    fn default() -> Self {
        Self {
            mmproj_url: String::new(),
            mmproj_path: String::new(),
            acceleration: ProactiveAcceleration::Auto,
        }
    }
}

impl LlamaCppPluginConfig {
    /// Reads the llama.cpp plugin config from a specific config document.
    #[must_use]
    pub fn from_config(config: &EneConfig) -> Self {
        match plugin_config_blob(config, LLAMA_CPP_PLUGIN) {
            Some(blob) => serde_json::from_value(blob).unwrap_or_else(|err| {
                // A malformed blob silently disabling the loader settings
                // (mmproj, acceleration) is hard to diagnose; surface the
                // parse error so the typo that produced it shows up in the
                // log stream instead of defaulting quietly.
                tracing::warn!(
                    component = "Ai",
                    plugin = LLAMA_CPP_PLUGIN,
                    error = %err,
                    "Failed to parse plugin config blob; using defaults"
                );
                Self::default()
            }),
            None => Self::default(),
        }
    }

    /// Reads the llama.cpp plugin config from the global config singleton.
    #[must_use]
    pub fn global() -> Self {
        Self::from_config(&ene_config::get_global_config())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_llama_cpp() -> EneConfig {
        let mut config = EneConfig::default();
        drop(config.set_path(
            "plugins.list.llama-cpp.config.mmproj_url",
            "https://cdn.example/mmproj.gguf",
        ));
        drop(config.set_path(
            "plugins.list.llama-cpp.config.mmproj_path",
            "/data/mmproj.gguf",
        ));
        drop(config.set_path("plugins.list.llama-cpp.config.acceleration", "\"vulkan\""));
        drop(config.set_path(
            "plugins.list.kokoro.profiles.kokoro.voices_path",
            "/data/voices.bin",
        ));
        config
    }

    #[test]
    fn llama_cpp_config_reads_nested_blob() {
        let cfg = LlamaCppPluginConfig::from_config(&config_with_llama_cpp());
        assert_eq!(cfg.mmproj_url, "https://cdn.example/mmproj.gguf");
        assert_eq!(cfg.mmproj_path, "/data/mmproj.gguf");
        assert_eq!(cfg.acceleration, ProactiveAcceleration::Vulkan);
    }

    #[test]
    fn llama_cpp_config_defaults_when_absent() {
        let cfg = LlamaCppPluginConfig::from_config(&EneConfig::default());
        assert!(cfg.mmproj_url.is_empty());
        assert!(cfg.mmproj_path.is_empty());
        assert_eq!(cfg.acceleration, ProactiveAcceleration::Auto);
    }

    #[test]
    fn profile_blob_reads_nested_profiles() {
        let blob = plugin_profile_blob(&config_with_llama_cpp(), KOKORO_PLUGIN, "kokoro")
            .expect("kokoro profile present");
        assert_eq!(blob["voices_path"], serde_json::json!("/data/voices.bin"));
        assert!(plugin_profile_blob(&config_with_llama_cpp(), KOKORO_PLUGIN, "nope").is_none());
    }
}
