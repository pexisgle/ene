//! Minimal sidecar configuration shared by the lifecycle and presets.
//!
//! The scaffolding plugin's real config type should expose the same fields;
//! merge this struct into `crate::config` (or re-export it from there) when
//! scaffolding a new sidecar provider.

use std::time::Duration;

use serde::Deserialize;

/// Default sidecar startup timeout when `startup_timeout_secs` is omitted.
pub const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 60;

/// Host-delivered sidecar configuration
/// (`plugins.list.__SIDECAR_NAME__.config`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SidecarConfig {
    /// Explicit engine binary path. When empty the resolver checks the
    /// bundled plugins directory, then `PATH`.
    pub server_path: Option<String>,
    /// How long the lifecycle waits for the health probe after spawning.
    pub startup_timeout_secs: Option<u64>,
    /// Model profiles serialized into the engine preset file.
    pub profiles: Vec<SidecarProfile>,
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

/// One model profile for the engine preset file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SidecarProfile {
    /// Profile key used by the engine preset.
    pub name: String,
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
