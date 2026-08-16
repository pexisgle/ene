use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use ene_ai::config::{GpuLayers, ProactiveAcceleration};
use ene_plugin::{PluginError, ResourceClass};
use serde::Deserialize;
use serde_json::Value;

/// Configuration delivered by the host at handshake time
/// (`plugins.list.llama-server.config`), stored per process.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// Per-profile configuration (`plugins.list.llama-server.profiles`), stored
/// per process. Profile *selection* is plugin-owned: each inference request
/// names a profile key, and the model for that key is loaded lazily on first
/// use.
static PLUGIN_PROFILES: Mutex<Option<Value>> = Mutex::new(None);

/// Default context window for a profile that omits `context_size`.
///
/// Matches `ene_ai::LocalModelDef`'s default and the `ene-plugin-llama-cpp`
/// plugin, so a config migrated from `plugins.list.llama-cpp.profiles`
/// behaves identically.
const DEFAULT_CONTEXT_SIZE: u32 = 16_384;

/// Default quantization label when a profile omits it (same as
/// `ene_ai::LocalModelDef`).
const DEFAULT_QUANTIZATION: &str = "F16";

/// How long the plugin waits for the sidecar to answer `/health` after
/// spawning it.
const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct HostConfig {
    /// Explicit path to the `llama-server` executable. When empty, the
    /// plugin looks beside its own binary and then on `PATH`.
    pub(crate) server_path: Option<String>,
    /// Extra command-line arguments passed to the sidecar on spawn.
    pub(crate) server_args: Vec<String>,
    pub(crate) startup_timeout_secs: Option<u64>,
    pub(crate) mmproj_url: Option<String>,
    pub(crate) mmproj_path: Option<String>,
    pub(crate) acceleration: Option<String>,
}

impl HostConfig {
    /// The effective acceleration backend, defaulting to `auto`.
    pub(crate) fn acceleration(&self) -> Result<ProactiveAcceleration, PluginError> {
        match self.acceleration.as_deref().map_or("auto", str::trim) {
            "" | "auto" => Ok(ProactiveAcceleration::Auto),
            "cpu" => Ok(ProactiveAcceleration::Cpu),
            "vulkan" => Ok(ProactiveAcceleration::Vulkan),
            "cuda" => Ok(ProactiveAcceleration::Cuda),
            other => Err(PluginError::provider(format!(
                "unknown acceleration backend: {other:?}"
            ))),
        }
    }

    /// Sidecar startup timeout in seconds (minimum 1).
    pub(crate) fn startup_timeout_secs(&self) -> u64 {
        self.startup_timeout_secs
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECS)
            .max(1)
    }
}

/// The [`ResourceClass`] this plugin's provider jobs contend on, derived from
/// the configured acceleration preference.
///
/// `auto` declares `Cpu`: unlike the in-process plugin, this binary has no
/// compile-time knowledge of the sidecar's GPU backends, so the conservative
/// reading is used unless the user explicitly asks for a GPU backend.
pub(crate) fn resource_class() -> Result<ResourceClass, PluginError> {
    current_config()?
        .acceleration()
        .map(declared_resource_class)
}

/// Pure acceleration → class mapping (also used by tests).
pub(crate) fn declared_resource_class(acceleration: ProactiveAcceleration) -> ResourceClass {
    match acceleration {
        ProactiveAcceleration::Cpu | ProactiveAcceleration::Auto => ResourceClass::Cpu,
        ProactiveAcceleration::Vulkan | ProactiveAcceleration::Cuda => {
            ResourceClass::Gpu { device: 0 }
        }
    }
}

/// Number of GPU layers to request from the sidecar.
///
/// `cpu` forces 0; anything else uses the profile's `gpu_layers` with the
/// same `auto` → all-layers convention as [`GpuLayers::n_layers`].
pub(crate) fn n_gpu_layers_for(acceleration: ProactiveAcceleration, gpu_layers: GpuLayers) -> u32 {
    match acceleration {
        ProactiveAcceleration::Cpu => 0,
        ProactiveAcceleration::Auto
        | ProactiveAcceleration::Vulkan
        | ProactiveAcceleration::Cuda => gpu_layers.n_layers(),
    }
}

/// One model profile (`plugins.list.llama-server.profiles.<name>`), mirroring
/// `ene_ai::LocalModelDef` and the `ene-plugin-llama-cpp` profile shape.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct Profile {
    pub(crate) url: Option<String>,
    pub(crate) artifact_id: Option<String>,
    pub(crate) artifact_version: Option<String>,
    pub(crate) quantization: Option<String>,
    pub(crate) model_path: Option<String>,
    pub(crate) gpu_layers: Option<GpuLayers>,
    pub(crate) context_size: Option<u32>,
    pub(crate) dimensions: Option<usize>,
}

impl Profile {
    /// Non-empty `model_path` override, if any.
    pub(crate) fn model_path(&self) -> Option<&str> {
        self.model_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
    }

    /// Non-empty download URL, if any.
    pub(crate) fn url(&self) -> Option<&str> {
        self.url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
    }

    /// Catalog artifact id for the weights, when catalog-managed.
    pub(crate) fn artifact_id(&self) -> Option<&str> {
        self.artifact_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    /// Optional catalog version pin.
    pub(crate) fn artifact_version(&self) -> Option<&str> {
        self.artifact_version
            .as_deref()
            .map(str::trim)
            .filter(|version| !version.is_empty())
    }

    /// Quantization label (e.g. `"F16"`, `"Q4_0"`).
    pub(crate) fn quantization(&self) -> &str {
        self.quantization
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(DEFAULT_QUANTIZATION)
    }

    /// GPU layer offload, defaulting to `auto`.
    pub(crate) fn gpu_layers(&self) -> GpuLayers {
        self.gpu_layers.unwrap_or_default()
    }

    /// Context window for chat loads.
    pub(crate) fn context_size(&self) -> u32 {
        self.context_size.unwrap_or(DEFAULT_CONTEXT_SIZE)
    }

    /// Declared embedding dimensionality, if any (the host needs it to open
    /// the memory-store vector schema; the plugin validates it against the
    /// sidecar's real output dimensions).
    pub(crate) fn dimensions(&self) -> Option<usize> {
        self.dimensions
    }
}

/// Stores the config blob delivered by [`crate::plugin::LlamaServerPlugin`]'s
/// `ConfigurablePlugin::set_config`.
pub(crate) fn set_config(config: &Value) {
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    super::models::config_changed();
}

/// Stores the profile map delivered by
/// [`crate::plugin::LlamaServerPlugin`]'s `ConfigurablePlugin::set_profiles`.
pub(crate) fn set_profiles(profiles: &Value) {
    *PLUGIN_PROFILES
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(profiles.clone());
    super::models::config_changed();
}

/// Parses the current host config, defaulting to empty when the handshake
/// delivered none (tests may call handlers directly).
pub(crate) fn current_config() -> Result<HostConfig, PluginError> {
    let Some(value) = PLUGIN_CONFIG
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .cloned()
    else {
        return Ok(HostConfig::default());
    };
    HostConfig::deserialize(value)
        .map_err(|e| PluginError::provider(format!("invalid plugin config: {e}")))
}

/// Looks up and parses the profile for `model` (a `local_models` key).
pub(crate) fn profile_for(model: &str) -> Result<Profile, PluginError> {
    let profiles = PLUGIN_PROFILES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(profiles) = profiles else {
        return Err(no_profile_error(model));
    };
    let Some(raw) = profiles.get(model) else {
        return Err(no_profile_error(model));
    };
    Profile::deserialize(raw.clone())
        .map_err(|e| PluginError::provider(format!("invalid profile {model:?}: {e}")))
}

/// Parses every profile, skipping malformed entries with a warning (they
/// surface as typed errors when a request names them). Used to build the
/// sidecar preset file.
pub(crate) fn all_profiles() -> HashMap<String, Profile> {
    let Some(profiles) = PLUGIN_PROFILES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .cloned()
    else {
        return HashMap::new();
    };
    let Some(object) = profiles.as_object() else {
        return HashMap::new();
    };
    let mut parsed = HashMap::new();
    for (name, raw) in object {
        match Profile::deserialize(raw.clone()) {
            Ok(profile) => {
                parsed.insert(name.clone(), profile);
            }
            Err(e) => {
                tracing::warn!(
                    component = "LlamaServerPlugin",
                    profile = name,
                    error = %e,
                    "skipping malformed profile in sidecar presets"
                );
            }
        }
    }
    parsed
}

fn no_profile_error(model: &str) -> PluginError {
    PluginError::provider(format!(
        "no profile configured for model {model:?} under plugins.list.llama-server.profiles"
    ))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn profile_defaults_match_local_model_def() {
        let profile: Profile =
            serde_json::from_value(serde_json::json!({})).expect("empty profile parses");
        assert_eq!(profile.model_path(), None);
        assert_eq!(profile.url(), None);
        assert_eq!(profile.quantization(), "F16");
        assert_eq!(profile.gpu_layers(), GpuLayers::Auto);
        assert_eq!(profile.context_size(), 16_384);
    }

    #[test]
    fn profile_accepts_mirror_fields() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "url": "https://cdn.example/model.gguf",
            "quantization": "Q4_0",
            "model_path": "/data/model.gguf",
            "gpu_layers": "33",
            "context_size": 4096,
            "dimensions": 384,
            "future_key": "preserved"
        }))
        .expect("profile parses");
        assert_eq!(profile.url(), Some("https://cdn.example/model.gguf"));
        assert_eq!(profile.quantization(), "Q4_0");
        assert_eq!(profile.model_path(), Some("/data/model.gguf"));
        assert_eq!(profile.gpu_layers(), GpuLayers::Layers(33));
        assert_eq!(profile.context_size(), 4096);
        assert_eq!(profile.dimensions(), Some(384));
    }

    #[test]
    fn resource_class_follows_acceleration() {
        assert_eq!(
            declared_resource_class(ProactiveAcceleration::Cpu),
            ResourceClass::Cpu
        );
        assert_eq!(
            declared_resource_class(ProactiveAcceleration::Auto),
            ResourceClass::Cpu
        );
        assert_eq!(
            declared_resource_class(ProactiveAcceleration::Vulkan),
            ResourceClass::Gpu { device: 0 }
        );
        assert_eq!(
            declared_resource_class(ProactiveAcceleration::Cuda),
            ResourceClass::Gpu { device: 0 }
        );
    }

    #[test]
    fn gpu_layer_mapping_matches_in_process_plugin() {
        assert_eq!(
            n_gpu_layers_for(ProactiveAcceleration::Cpu, GpuLayers::Layers(33)),
            0
        );
        assert_eq!(
            n_gpu_layers_for(ProactiveAcceleration::Vulkan, GpuLayers::Layers(33)),
            33
        );
        assert_eq!(
            n_gpu_layers_for(ProactiveAcceleration::Auto, GpuLayers::Auto),
            999
        );
    }

    #[test]
    fn profile_rejects_invalid_gpu_layers() {
        let result: Result<Profile, _> =
            serde_json::from_value(serde_json::json!({ "gpu_layers": "bogus" }));
        assert!(
            result.is_err(),
            "misconfiguration must fail at the boundary"
        );
    }

    #[test]
    fn startup_timeout_defaults_and_clamps() {
        let cfg = HostConfig::default();
        assert_eq!(cfg.startup_timeout_secs(), 60);
        let cfg = HostConfig::deserialize(serde_json::json!({"startup_timeout_secs": 0}))
            .expect("config parses");
        assert_eq!(cfg.startup_timeout_secs(), 1);
    }
}
