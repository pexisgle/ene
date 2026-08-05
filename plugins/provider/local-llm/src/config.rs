//! Host-delivered config and per-model profiles for the local GGUF provider.

use std::sync::{Mutex, OnceLock, PoisonError, atomic::AtomicU64};

use ene_ai::config::ProactiveAcceleration;
use ene_plugin::{PluginError, ResourceClass};
use serde::Deserialize;
use serde_json::Value;

/// Configuration delivered by the host at handshake time
/// (`plugins.list.llama-cpp.config`), stored per process.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// Per-profile configuration (`plugins.list.llama-cpp.profiles`), stored per
/// process. Profile *selection* is plugin-owned: each inference request names
/// a profile key, and the model for that key is loaded lazily on first use.
static PLUGIN_PROFILES: Mutex<Option<Value>> = Mutex::new(None);

/// Incremented whenever the host pushes config or profiles. Model loads carry
/// the generation they started under, so a live update cannot publish a stale
/// model into the new configuration's cache.
static CONFIG_GENERATION: OnceLock<AtomicU64> = OnceLock::new();

/// Default context window for a profile that omits `context_size`.
///
/// Matches `ene_ai::LocalModelDef`'s default. The v2→v3 migration mirrors
/// `ai.local_models.<name>.context_size` into profiles, so an omitted field
/// means the host value was also left at the default.
const DEFAULT_CONTEXT_SIZE: u32 = 16_384;

/// Default quantization label when a profile omits it (same as
/// `ene_ai::LocalModelDef`).
const DEFAULT_QUANTIZATION: &str = "F16";

/// Default GPU offload when a profile omits it (same as
/// `ene_ai::LocalModelDef`).
const DEFAULT_GPU_LAYERS: &str = "auto";

/// The host-delivered plugin config blob (`plugins.list.llama-cpp.config`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct HostConfig {
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
}

/// The [`ResourceClass`] this plugin's provider jobs contend on, derived from
/// the configured acceleration preference and the compiled GPU backend.
///
/// The wire declaration is per provider kind, while the actual offload is
/// per model profile (`gpu_layers`); this maps the configured *intent* and
/// errs toward `Gpu` — a profile forced to CPU under a GPU acceleration
/// over-declares safely, while under-declaring would bypass the host's
/// per-class admission entirely.
pub(crate) fn resource_class() -> Result<ResourceClass, PluginError> {
    current_config()?
        .acceleration()
        .map(declared_resource_class)
}

/// Pure acceleration → class mapping (also used by tests).
pub(crate) fn declared_resource_class(acceleration: ProactiveAcceleration) -> ResourceClass {
    match acceleration {
        ProactiveAcceleration::Cpu => ResourceClass::Cpu,
        ProactiveAcceleration::Vulkan | ProactiveAcceleration::Cuda => {
            ResourceClass::Gpu { device: 0 }
        }
        ProactiveAcceleration::Auto => {
            if cfg!(feature = "vulkan") || cfg!(feature = "cuda") {
                ResourceClass::Gpu { device: 0 }
            } else {
                ResourceClass::Cpu
            }
        }
    }
}

/// One model profile (`plugins.list.llama-cpp.profiles.<name>`), mirroring
/// `ene_ai::LocalModelDef` (the v2→v3 migration mirrors every field).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct Profile {
    pub(crate) url: Option<String>,
    pub(crate) quantization: Option<String>,
    pub(crate) model_path: Option<String>,
    pub(crate) gpu_layers: Option<String>,
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

    /// Quantization label (e.g. `"F16"`, `"Q4_0"`).
    pub(crate) fn quantization(&self) -> &str {
        self.quantization
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(DEFAULT_QUANTIZATION)
    }

    /// GPU layer offload: `"auto"` or an integer string.
    pub(crate) fn gpu_layers(&self) -> &str {
        self.gpu_layers
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_GPU_LAYERS)
    }

    /// Context window for chat loads (the embedding provider sizes its own
    /// context internally).
    pub(crate) fn context_size(&self) -> u32 {
        self.context_size.unwrap_or(DEFAULT_CONTEXT_SIZE)
    }

    /// Declared embedding dimensionality, if any (the model's real `n_embd`
    /// is measured at load and compared against it host-side).
    pub(crate) fn dimensions(&self) -> Option<usize> {
        self.dimensions
    }
}

/// Stores the config blob delivered by [`crate::plugin::LocalLlmPlugin`]'s
/// `ConfigurablePlugin::set_config`.
pub(crate) fn set_config(config: &Value) {
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    super::models::config_changed();
}

/// Stores the profile map delivered by
/// [`crate::plugin::LocalLlmPlugin`]'s `ConfigurablePlugin::set_profiles`.
pub(crate) fn set_profiles(profiles: &Value) {
    *PLUGIN_PROFILES
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(profiles.clone());
    super::models::config_changed();
}

/// Returns the current host-configuration generation.
pub(crate) fn generation() -> u64 {
    CONFIG_GENERATION
        .get_or_init(|| AtomicU64::new(0))
        .load(std::sync::atomic::Ordering::Acquire)
}

/// Advances the host-configuration generation while model-cache locks are
/// held by [`super::models::config_changed`].
pub(crate) fn advance_generation() {
    CONFIG_GENERATION
        .get_or_init(|| AtomicU64::new(0))
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
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

fn no_profile_error(model: &str) -> PluginError {
    PluginError::provider(format!(
        "no profile configured for model {model:?} under plugins.list.llama-cpp.profiles"
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
        assert_eq!(profile.gpu_layers(), "auto");
        assert_eq!(profile.context_size(), 16_384);
    }

    #[test]
    fn profile_accepts_slice_b_mirror_fields() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "url": "https://cdn.example/model.gguf",
            "quantization": "Q4_0",
            "model_path": "/data/model.gguf",
            "gpu_layers": "33",
            "context_size": 8192,
        }))
        .expect("profile parses");
        assert_eq!(profile.url(), Some("https://cdn.example/model.gguf"));
        assert_eq!(profile.model_path(), Some("/data/model.gguf"));
        assert_eq!(profile.quantization(), "Q4_0");
        assert_eq!(profile.gpu_layers(), "33");
        assert_eq!(profile.context_size(), 8192);
    }

    #[test]
    fn profile_trims_blank_override_fields() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "model_path": "  ",
            "quantization": " ",
        }))
        .expect("profile parses");
        assert_eq!(profile.model_path(), None);
        assert_eq!(profile.quantization(), "F16");
    }

    #[test]
    fn invalid_profile_shape_is_typed_error() {
        let err = serde_json::from_value::<Profile>(serde_json::json!("not-an-object"))
            .expect_err("rejects scalar");
        assert!(err.is_data());
    }

    #[test]
    fn acceleration_mapping_covers_all_backends() {
        let config: HostConfig =
            serde_json::from_value(serde_json::json!({"acceleration": "vulkan"}))
                .expect("config parses");
        assert!(matches!(
            config.acceleration().expect("valid"),
            ProactiveAcceleration::Vulkan
        ));
        let auto: HostConfig =
            serde_json::from_value(serde_json::json!({})).expect("empty config parses");
        assert!(matches!(
            auto.acceleration().expect("defaults to auto"),
            ProactiveAcceleration::Auto
        ));
        let bad: HostConfig = serde_json::from_value(serde_json::json!({
            "acceleration": "metal"
        }))
        .expect("config parses");
        assert!(bad.acceleration().is_err());
    }

    #[test]
    fn declared_resource_class_matches_acceleration_and_build() {
        use ene_plugin::ResourceClass;

        assert_eq!(
            declared_resource_class(ProactiveAcceleration::Cpu),
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
        // Auto mirrors the load-time backend selection: GPU when a backend
        // is compiled in, CPU otherwise.
        let expected = if cfg!(feature = "vulkan") || cfg!(feature = "cuda") {
            ResourceClass::Gpu { device: 0 }
        } else {
            ResourceClass::Cpu
        };
        assert_eq!(
            declared_resource_class(ProactiveAcceleration::Auto),
            expected
        );
    }

    #[test]
    fn resource_class_reads_the_delivered_config() {
        use ene_plugin::ResourceClass;

        set_config(&serde_json::json!({"acceleration": "vulkan"}));
        assert_eq!(
            resource_class().expect("vulkan config maps to gpu"),
            ResourceClass::Gpu { device: 0 }
        );
        set_config(&serde_json::json!({"acceleration": "cpu"}));
        assert_eq!(
            resource_class().expect("cpu config maps to cpu"),
            ResourceClass::Cpu
        );
        // Unknown backend is a typed error, not a silent declaration.
        set_config(&serde_json::json!({"acceleration": "metal"}));
        assert!(resource_class().is_err());
        set_config(&serde_json::json!({}));
    }
}
