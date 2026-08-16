//! Plugin configuration (`plugins.list.openai-tts.config`).

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_MODEL: &str = "tts-1";
/// Default voice when neither the request nor the config names one.
pub const DEFAULT_VOICE: &str = "alloy";
pub const DEFAULT_SPEED: f32 = 1.0;
/// Default output sample rate (the Speech API's `pcm` format is fixed).
pub const DEFAULT_SAMPLE_RATE: u32 = 24_000;
/// Largest sample rate whose 16-bit mono WAV byte rate fits in RIFF's u32 field.
pub const MAX_SAMPLE_RATE: u32 = u32::MAX / 2;
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const MIN_SPEED: f32 = 0.25;
pub const MAX_SPEED: f32 = 4.0;
/// Models advertised via `tts_spec` and accepted at runtime.
pub const SUPPORTED_MODELS: &[&str] = &["tts-1", "tts-1-hd"];
/// Voices advertised via `tts_spec`; the API validates them per request.
pub const SUPPORTED_VOICES: &[&str] = &["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

/// Settings for the `OpenAI Speech API` TTS provider.
///
/// `api_key` is deliberately not part of the struct: the host resolves it
/// into the credential store and injects it at request time, so the secret
/// never round-trips through the plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct OpenAiTtsConfig {
    /// Speech synthesis model (`tts-1` / `tts-1-hd`).
    pub model: String,
    /// Default voice; a per-request voice overrides it.
    pub voice: String,
    /// Speech speed multiplier (0.25–4.0).
    pub speed: f32,
    /// Output sample rate written into the WAV header (the Speech API's
    /// `pcm` format is fixed at 24 kHz; override for compatible endpoints).
    pub sample_rate: u32,
    pub base_url: String,
}

impl Default for OpenAiTtsConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            speed: DEFAULT_SPEED,
            sample_rate: DEFAULT_SAMPLE_RATE,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }
}

impl OpenAiTtsConfig {
    /// Parses the provider config blob delivered with a synthesize request.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the blob is not a JSON object, a field
    /// has the wrong type, or a value is outside the API's contracts
    /// (`speed` 0.25–4.0, `sample_rate` non-zero, known `model` / `voice`).
    pub fn from_value(value: &Value) -> Result<Self, PluginError> {
        let config: Self = serde_json::from_value(value.clone()).map_err(|e| {
            PluginError::provider(format!("invalid openai-tts provider config: {e}"))
        })?;
        if !(MIN_SPEED..=MAX_SPEED).contains(&config.speed) {
            return Err(PluginError::provider(format!(
                "invalid openai-tts provider config: speed {} is outside the \
                 {MIN_SPEED}-{MAX_SPEED} range",
                config.speed
            )));
        }
        if config.sample_rate == 0 || config.sample_rate > MAX_SAMPLE_RATE {
            return Err(PluginError::provider(format!(
                "invalid openai-tts provider config: sample_rate must be between 1 and {MAX_SAMPLE_RATE}"
            )));
        }
        if config.model.is_empty() || !SUPPORTED_MODELS.contains(&config.model.as_str()) {
            return Err(PluginError::provider(format!(
                "invalid openai-tts provider config: unknown model {:?}; \
                 expected one of {SUPPORTED_MODELS:?}",
                config.model
            )));
        }
        if !config.voice.is_empty() && !SUPPORTED_VOICES.contains(&config.voice.as_str()) {
            return Err(PluginError::provider(format!(
                "invalid openai-tts provider config: unknown voice {:?}; \
                 expected one of {SUPPORTED_VOICES:?}",
                config.voice
            )));
        }
        Ok(config)
    }

    /// Resolves the voice for a request: a non-empty request voice wins,
    /// then the configured voice, then the API default.
    #[must_use]
    pub fn resolve_voice(&self, request_voice: &str) -> String {
        if !request_voice.trim().is_empty() {
            request_voice.trim().to_string()
        } else if !self.voice.trim().is_empty() {
            self.voice.trim().to_string()
        } else {
            DEFAULT_VOICE.to_string()
        }
    }
}

/// Resolves the effective API base URL with the same precedence as the key:
/// host blob, request config, `OPENAI_BASE_URL`, then the API default.
#[must_use]
pub fn resolve_base_url(host_config: Option<&Value>, config: &Value) -> String {
    host_config
        .as_ref()
        .and_then(|cfg| cfg.get("base_url"))
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map_or_else(
            || {
                std::env::var("OPENAI_BASE_URL")
                    .ok()
                    .map(|url| url.trim().to_string())
                    .filter(|url| !url.is_empty())
                    .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            },
            str::to_string,
        )
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = OpenAiTtsConfig::from_value(&json!({})).expect("empty config parses");
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.voice, DEFAULT_VOICE);
        assert!((cfg.speed - 1.0).abs() < 1e-6);
        assert_eq!(cfg.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn parses_all_fields_and_ignores_unknown() {
        let cfg = OpenAiTtsConfig::from_value(&json!({
            "model": "tts-1-hd",
            "voice": "nova",
            "speed": 1.5,
            "sample_rate": 48_000,
            "base_url": "https://example.com/v1",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(cfg.model, "tts-1-hd");
        assert_eq!(cfg.voice, "nova");
        assert!((cfg.speed - 1.5).abs() < 1e-6);
        assert_eq!(cfg.sample_rate, 48_000);
        assert_eq!(cfg.base_url, "https://example.com/v1");
    }

    #[test]
    fn rejects_wrong_field_types() {
        let err = OpenAiTtsConfig::from_value(&json!({"speed": "fast"}))
            .expect_err("wrong type rejected");
        assert!(err.to_string().contains("provider"));
    }

    #[test]
    fn rejects_speed_outside_api_range() {
        for speed in [0.2, -1.0, 4.1, 100.0] {
            let err = OpenAiTtsConfig::from_value(&json!({"speed": speed}))
                .expect_err("out-of-range speed rejected");
            assert!(err.to_string().contains("0.25"));
        }
    }

    #[test]
    fn rejects_zero_sample_rate() {
        let err = OpenAiTtsConfig::from_value(&json!({"sample_rate": 0}))
            .expect_err("zero sample rate rejected");
        assert!(err.to_string().contains("sample_rate"));

        let err = OpenAiTtsConfig::from_value(&json!({
            "sample_rate": u64::from(MAX_SAMPLE_RATE) + 1
        }))
        .expect_err("sample rate with an overflowing byte rate rejected");
        assert!(err.to_string().contains("sample_rate"));
    }

    #[test]
    fn rejects_unknown_model_and_voice() {
        let err = OpenAiTtsConfig::from_value(&json!({"model": "tts-2"}))
            .expect_err("unknown model rejected");
        assert!(err.to_string().contains("model"));
        let err = OpenAiTtsConfig::from_value(&json!({"voice": "clippy"}))
            .expect_err("unknown voice rejected");
        assert!(err.to_string().contains("voice"));
    }

    #[test]
    fn empty_voice_falls_back_to_default() {
        let cfg = OpenAiTtsConfig::from_value(&json!({"voice": ""})).expect("empty voice parses");
        assert_eq!(cfg.resolve_voice(""), DEFAULT_VOICE);
    }

    #[test]
    fn accepts_speed_boundaries() {
        for speed in [0.25, 4.0] {
            let cfg =
                OpenAiTtsConfig::from_value(&json!({"speed": speed})).expect("boundary accepted");
            assert!((cfg.speed - speed).abs() < 1e-6);
        }
    }

    #[test]
    fn request_voice_wins_then_config_then_default() {
        let cfg = OpenAiTtsConfig::from_value(&json!({"voice": "echo"})).expect("config parses");
        assert_eq!(cfg.resolve_voice("shimmer"), "shimmer");
        assert_eq!(cfg.resolve_voice(""), "echo");
        let default = OpenAiTtsConfig::default();
        assert_eq!(default.resolve_voice(""), DEFAULT_VOICE);
    }
}
