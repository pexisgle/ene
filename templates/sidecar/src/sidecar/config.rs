//! Minimal sidecar configuration shared by the lifecycle and presets.
//!
//! The scaffolding plugin's real config type should expose the same fields;
//! merge this struct into `crate::config` (or re-export it from there) when
//! scaffolding a new sidecar provider.
//!
//! The host delivers the two inputs separately: `ENE_PROVIDER_CONFIG` carries
//! this `SidecarConfig` JSON, while `set_profiles` receives the per-model
//! profile map ([`SidecarProfiles`]). Combine both before writing engine presets.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

/// Default sidecar startup timeout when `startup_timeout_secs` is omitted.
pub const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 60;

/// Host-delivered sidecar configuration (`ENE_PROVIDER_CONFIG`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SidecarConfig {
    /// Explicit engine binary path. When empty the resolver checks the
    /// bundled plugins directory, then `PATH`.
    pub server_path: Option<String>,
    /// How long the lifecycle waits for the health probe after spawning.
    pub startup_timeout_secs: Option<u64>,
}

impl SidecarConfig {
    /// Effective startup timeout, clamped to at least one second.
    #[must_use]
    pub fn startup_timeout(&self) -> Duration {
        Duration::from_secs(
            self.startup_timeout_secs
                .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECS)
                .max(1),
        )
    }
}

/// Per-model profiles delivered separately by the host's `set_profiles`
/// handshake, keyed by profile name.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct SidecarProfiles(pub BTreeMap<String, SidecarProfile>);

/// One model profile for the engine preset file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SidecarProfile {
    /// GGUF / model path (host-injected `model_path` or downloaded weights).
    pub model_path: String,
    /// GPU layer offload: `"auto"` or a layer count.
    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: String,
    /// Context window in tokens.
    pub context_size: Option<u32>,
}

fn default_gpu_layers() -> String {
    "auto".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_set_config_blob_without_profiles() {
        let config: SidecarConfig = serde_json::from_value(serde_json::json!({
            "server_path": "/usr/bin/engine",
            "startup_timeout_secs": 30,
        }))
        .expect("config blob shape");
        assert_eq!(config.server_path.as_deref(), Some("/usr/bin/engine"));
        assert_eq!(config.startup_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn profiles_parse_set_profiles_map() {
        let profiles: SidecarProfiles = serde_json::from_value(serde_json::json!({
            "chat": {
                "model_path": "/data/chat.gguf",
                "gpu_layers": "auto",
                "context_size": 4096,
            }
        }))
        .expect("set_profiles shape");
        let chat = profiles.0.get("chat").expect("profile");
        assert_eq!(chat.model_path, "/data/chat.gguf");
        assert_eq!(chat.gpu_layers, "auto");
        assert_eq!(chat.context_size, Some(4096));
    }
}
