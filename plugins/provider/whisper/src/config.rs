//! Plugin configuration (`plugins.list.whisper.config`) and model-path
//! resolution for the local whisper.cpp STT provider.

use std::path::PathBuf;

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Settings for the local whisper.cpp STT provider.
///
/// `model` and `language` are forwarded per request from `ai.stt.*` by the
/// host adapter (mirroring how the TTS adapter forwards `ai.tts.voice`), so
/// only the path override lives in the plugin blob.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct WhisperConfig {
    /// Explicit whisper GGUF model file path.
    pub model_path: Option<String>,
    /// Engine mode: `auto` (sidecar when a server binary is configured,
    /// otherwise in-process), `sidecar` (require the whisper-server child),
    /// or `in-process` (never spawn a child).
    pub mode: Option<String>,
    /// Path to the `whisper-server` executable (host-injected when a
    /// catalog-managed sidecar artifact is installed).
    pub server_path: Option<String>,
    /// Extra command-line arguments passed to the sidecar on spawn.
    pub server_args: Vec<String>,
    /// How long to wait for the sidecar health check after spawning.
    pub startup_timeout_secs: Option<u64>,
}

/// Effective engine mode after resolving the `mode` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineMode {
    /// Use the sidecar when a server path is configured; fall back to the
    /// in-process engine otherwise.
    Auto,
    /// Always transcribe through the whisper-server sidecar.
    Sidecar,
    /// Always use the in-process whisper.cpp engine.
    InProcess,
}

impl WhisperConfig {
    /// Sidecar startup timeout in seconds (minimum 1).
    #[must_use]
    pub fn startup_timeout_secs(&self) -> u64 {
        self.startup_timeout_secs.unwrap_or(60).max(1)
    }

    /// Resolves the configured engine mode.
    #[must_use]
    pub fn mode(&self) -> EngineMode {
        match self.mode.as_deref().map_or("auto", str::trim) {
            "sidecar" => EngineMode::Sidecar,
            "in-process" | "in_process" | "inprocess" => EngineMode::InProcess,
            _ => EngineMode::Auto,
        }
    }

    /// Whether this config wants the sidecar engine: explicit `sidecar`, or
    /// `auto` with a server path configured (the host injects one when a
    /// catalog-managed sidecar artifact is installed).
    #[must_use]
    pub fn wants_sidecar(&self) -> bool {
        match self.mode() {
            EngineMode::Sidecar => true,
            EngineMode::InProcess => false,
            EngineMode::Auto => non_empty(self.server_path.as_deref()).is_some(),
        }
    }
}

impl WhisperConfig {
    /// Parses the provider config blob delivered with a transcribe request.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the blob is not a JSON object or a
    /// field has the wrong type.
    pub fn from_value(value: &Value) -> Result<Self, PluginError> {
        serde_json::from_value(value.clone())
            .map_err(|e| PluginError::provider(format!("invalid whisper provider config: {e}")))
    }

    /// Resolves the GGUF model path with the same precedence the in-process
    /// engine used: `model_path`, then the per-request `model` name, then the
    /// shared cache.
    #[must_use]
    pub fn resolve_model_path(&self, request_model: &str) -> PathBuf {
        non_empty(self.model_path.as_deref())
            .map(PathBuf::from)
            .or_else(|| non_empty(Some(request_model)).map(PathBuf::from))
            .unwrap_or_else(|| ene_config::models_dir().join("gguf").join("whisper.gguf"))
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn model_path_precedence() {
        let config = WhisperConfig {
            model_path: Some("/data/whisper.gguf".into()),
            ..WhisperConfig::default()
        };
        assert_eq!(
            config.resolve_model_path("small.gguf"),
            PathBuf::from("/data/whisper.gguf")
        );
    }

    #[test]
    fn request_model_falls_back_to_cache() {
        let config = WhisperConfig::default();
        assert_eq!(
            config.resolve_model_path("small.gguf"),
            PathBuf::from("small.gguf")
        );
        assert_eq!(
            config.resolve_model_path(""),
            ene_config::models_dir().join("gguf").join("whisper.gguf")
        );
    }

    #[test]
    fn from_value_rejects_wrong_types() {
        let err = WhisperConfig::from_value(&Value::String("nope".into())).expect_err("not object");
        assert!(err.to_string().contains("invalid whisper provider config"));
    }
}
