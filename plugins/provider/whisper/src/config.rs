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
