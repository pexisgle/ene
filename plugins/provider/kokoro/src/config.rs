//! Plugin configuration (`plugins.list.kokoro.config`) and model-path
//! resolution.

use std::path::PathBuf;

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_SPEED: f32 = 1.0;
/// Smallest speed accepted, matching the desktop settings slider.
pub const MIN_SPEED: f32 = 0.5;
/// Largest speed accepted, matching the desktop settings slider.
pub const MAX_SPEED: f32 = 2.0;
/// Profile under `plugins.list.kokoro.profiles` that carries the legacy
/// `voices_path` slot (formerly `ai.tts.voices_path`, moved by the config
/// migration).
pub const DEFAULT_PROFILE: &str = "kokoro";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct KokoroConfig {
    /// ONNX model file path; defaults to the shared models cache.
    pub model_path: Option<String>,
    /// `voices.bin` path; falls back to the `kokoro` profile's
    /// `voices_path`, then the shared models cache.
    pub voices_path: Option<String>,
    /// Default voice; empty selects the first voice in `voices.bin`. A
    /// non-empty per-request voice overrides it.
    pub voice: String,
    /// Speech speed multiplier (0.5-2.0).
    pub speed: f32,
    /// G2P language: `"ja"` selects the Japanese kana rules, anything else
    /// the English rules.
    pub language: Option<String>,
    /// ONNX Runtime dynamic library path override (`ort` default resolution
    /// when unset).
    pub ort_dylib_path: Option<String>,
}

impl Default for KokoroConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            voices_path: None,
            voice: String::new(),
            speed: DEFAULT_SPEED,
            language: None,
            ort_dylib_path: None,
        }
    }
}

impl KokoroConfig {
    /// Parses the provider config blob delivered with a synthesize request.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the blob is not a JSON object, a field
    /// has the wrong type, or `speed` is outside the 0.5-2.0 range.
    pub fn from_value(value: &Value) -> Result<Self, PluginError> {
        let config: Self = serde_json::from_value(value.clone())
            .map_err(|e| PluginError::provider(format!("invalid kokoro provider config: {e}")))?;
        if !(MIN_SPEED..=MAX_SPEED).contains(&config.speed) {
            return Err(PluginError::provider(format!(
                "invalid kokoro provider config: speed {} is outside the \
                 {MIN_SPEED}-{MAX_SPEED} range",
                config.speed
            )));
        }
        Ok(config)
    }

    /// Resolves the effective model settings: a non-empty per-request voice
    /// wins over `config.voice`; `voices_path` prefers the config blob, then
    /// the `kokoro` profile (the migration target for the former
    /// `ai.tts.voices_path`), then the shared models cache.
    #[must_use]
    pub fn resolve(&self, request_voice: &str, profile: Option<&Value>) -> ResolvedConfig {
        let voice = if request_voice.trim().is_empty() {
            self.voice.trim().to_string()
        } else {
            request_voice.trim().to_string()
        };
        let voices_path = non_empty(self.voices_path.as_deref())
            .map(PathBuf::from)
            .or_else(|| {
                profile
                    .and_then(|p| p.get("voices_path"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(ene_voice::default_kokoro_voices_path);
        ResolvedConfig {
            model_path: non_empty(self.model_path.as_deref())
                .map_or_else(ene_voice::default_kokoro_model_path, PathBuf::from),
            voices_path,
            voice,
            speed: self.speed,
            language: self.language.clone(),
            ort_dylib_path: non_empty(self.ort_dylib_path.as_deref()).map(str::to_string),
        }
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// Fully-resolved model settings handed to the engine builder.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub model_path: PathBuf,
    pub voices_path: PathBuf,
    /// Possibly empty = first voice.
    pub voice: String,
    pub speed: f32,
    pub language: Option<String>,
    pub ort_dylib_path: Option<String>,
}

impl ResolvedConfig {
    /// The engine cache key: everything the loaded model depends on except
    /// `ort_dylib_path`, which ONNX Runtime fixes at first init
    /// (process-global, first caller wins) and therefore cannot be reloaded
    /// live.
    #[must_use]
    pub fn key(&self) -> EngineKey {
        EngineKey {
            model_path: self.model_path.clone(),
            voices_path: self.voices_path.clone(),
            voice: self.voice.clone(),
            speed: self.speed,
            language: self.language.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineKey {
    pub model_path: PathBuf,
    pub voices_path: PathBuf,
    pub voice: String,
    pub speed: f32,
    pub language: Option<String>,
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::float_cmp,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_config_resolves_to_defaults() {
        let cfg = KokoroConfig::from_value(&json!({})).expect("empty config parses");
        assert_eq!(cfg.speed, DEFAULT_SPEED);
        let resolved = cfg.resolve("", None);
        assert_eq!(resolved.model_path, ene_voice::default_kokoro_model_path());
        assert_eq!(
            resolved.voices_path,
            ene_voice::default_kokoro_voices_path()
        );
        assert_eq!(resolved.voice, "");
    }

    #[test]
    fn parses_all_fields_and_ignores_unknown() {
        let cfg = KokoroConfig::from_value(&json!({
            "model_path": "/data/kokoro.onnx",
            "voices_path": "/data/voices.bin",
            "voice": "af_bella",
            "speed": 1.25,
            "language": "ja",
            "ort_dylib_path": "/opt/ort/libonnxruntime.so",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(cfg.model_path.as_deref(), Some("/data/kokoro.onnx"));
        assert_eq!(cfg.voices_path.as_deref(), Some("/data/voices.bin"));
        assert_eq!(cfg.voice, "af_bella");
        assert!((cfg.speed - 1.25).abs() < 1e-6);
        assert_eq!(cfg.language.as_deref(), Some("ja"));
        assert_eq!(
            cfg.ort_dylib_path.as_deref(),
            Some("/opt/ort/libonnxruntime.so")
        );
    }

    #[test]
    fn rejects_wrong_field_types() {
        let err =
            KokoroConfig::from_value(&json!({"speed": "fast"})).expect_err("wrong type rejected");
        assert!(err.to_string().contains("provider"));
    }

    #[test]
    fn rejects_speed_outside_range() {
        for speed in [0.1, -1.0, 2.5, 100.0] {
            let err = KokoroConfig::from_value(&json!({"speed": speed}))
                .expect_err("out-of-range speed rejected");
            assert!(err.to_string().contains("0.5"));
        }
    }

    #[test]
    fn accepts_speed_boundaries() {
        for speed in [MIN_SPEED, MAX_SPEED] {
            let cfg =
                KokoroConfig::from_value(&json!({"speed": speed})).expect("boundary accepted");
            assert!((cfg.speed - speed).abs() < 1e-6);
        }
    }

    #[test]
    fn request_voice_wins_then_config_then_empty() {
        let cfg = KokoroConfig::from_value(&json!({"voice": "af_bella"})).expect("config parses");
        assert_eq!(cfg.resolve("jf_alpha", None).voice, "jf_alpha");
        assert_eq!(cfg.resolve("  ", None).voice, "af_bella");
        assert_eq!(KokoroConfig::default().resolve("", None).voice, "");
    }

    #[test]
    fn voices_path_prefers_config_then_profile_then_default() {
        let cfg = KokoroConfig::from_value(&json!({"voices_path": "/cfg/voices.bin"}))
            .expect("config parses");
        let profile = json!({"voices_path": "/profile/voices.bin"});
        assert_eq!(
            cfg.resolve("", Some(&profile)).voices_path,
            PathBuf::from("/cfg/voices.bin")
        );

        let cfg = KokoroConfig::default();
        assert_eq!(
            cfg.resolve("", Some(&profile)).voices_path,
            PathBuf::from("/profile/voices.bin")
        );
        assert_eq!(
            cfg.resolve("", None).voices_path,
            ene_voice::default_kokoro_voices_path()
        );
    }
}
