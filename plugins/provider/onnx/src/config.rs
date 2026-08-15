//! Plugin configuration (`plugins.list.onnx.config`) and model-path
//! resolution for the local ONNX provider plugin.

use std::path::PathBuf;

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default speech probability threshold, matching the former
/// `ai.vad.threshold` default.
pub const DEFAULT_THRESHOLD: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct VadConfig {
    /// Model name used as a path fallback when `model_path` is unset.
    pub model: String,
    /// Silero VAD ONNX model file path; falls back to `model`, then the
    /// shared models cache.
    pub model_path: Option<String>,
    /// Speech probability threshold (0.0-1.0).
    pub threshold: f32,
    /// ONNX Runtime dynamic library path override (`ort` default resolution
    /// when unset). Fixed at process start: ONNX Runtime initializes once,
    /// so a change requires a restart.
    pub ort_dylib_path: Option<String>,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            model_path: None,
            threshold: DEFAULT_THRESHOLD,
            ort_dylib_path: None,
        }
    }
}

impl VadConfig {
    /// Parses the provider config blob delivered with each `ProcessVadChunk`
    /// request.
    ///
    /// `threshold` is clamped to `[0.0, 1.0]` (with a warning when adjusted),
    /// matching the clamping the former `ai.vad.threshold` resolve applied.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the blob is not a JSON object or a
    /// field has the wrong type.
    pub fn from_value(value: &Value) -> Result<Self, PluginError> {
        let mut config: Self = serde_json::from_value(value.clone())
            .map_err(|e| PluginError::provider(format!("invalid onnx provider config: {e}")))?;
        let clamped = config.threshold.clamp(0.0, 1.0);
        if (clamped - config.threshold).abs() > f32::EPSILON {
            tracing::warn!(
                component = "ene-plugin-onnx",
                value = config.threshold,
                clamped,
                "VAD threshold out of range; clamping"
            );
            config.threshold = clamped;
        }
        Ok(config)
    }

    /// Resolves the ONNX model path with the same precedence the in-process
    /// Silero engine used: `model_path`, then `model`, then the shared cache.
    #[must_use]
    pub fn resolve_model_path(&self) -> PathBuf {
        non_empty(self.model_path.as_deref())
            .map(PathBuf::from)
            .or_else(|| non_empty(Some(self.model.as_str())).map(PathBuf::from))
            .unwrap_or_else(|| {
                ene_config::models_dir()
                    .join("gguf")
                    .join("silero_vad.onnx")
            })
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
        let config = VadConfig {
            model: "fallback.onnx".into(),
            model_path: Some("/data/silero.onnx".into()),
            ..VadConfig::default()
        };
        assert_eq!(
            config.resolve_model_path(),
            PathBuf::from("/data/silero.onnx")
        );
    }

    #[test]
    fn model_name_is_used_as_path_fallback() {
        let config = VadConfig {
            model: "my-vad.onnx".into(),
            ..VadConfig::default()
        };
        assert_eq!(config.resolve_model_path(), PathBuf::from("my-vad.onnx"));
    }

    #[test]
    fn empty_fields_use_default_cache_path() {
        let config = VadConfig::default();
        assert_eq!(
            config.resolve_model_path(),
            ene_config::models_dir()
                .join("gguf")
                .join("silero_vad.onnx")
        );
        assert!((config.threshold - DEFAULT_THRESHOLD).abs() < f32::EPSILON);
    }

    #[test]
    fn from_value_rejects_wrong_types() {
        let err = VadConfig::from_value(&Value::String("nope".into())).expect_err("not an object");
        assert!(err.to_string().contains("invalid onnx provider config"));
    }

    #[test]
    fn from_value_clamps_threshold() {
        let config = VadConfig::from_value(&serde_json::json!({"threshold": 2.5})).expect("parses");
        assert!((config.threshold - 1.0).abs() < f32::EPSILON);
        let config =
            VadConfig::from_value(&serde_json::json!({"threshold": -0.5})).expect("parses");
        assert!(config.threshold.abs() < f32::EPSILON);
    }
}
