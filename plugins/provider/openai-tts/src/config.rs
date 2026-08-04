//! Plugin configuration (`plugins.list.openai-tts.config`).

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default Speech API model (low latency).
pub const DEFAULT_MODEL: &str = "tts-1";
/// Default voice when neither the request nor the config names one.
pub const DEFAULT_VOICE: &str = "alloy";
/// Default speech speed multiplier.
pub const DEFAULT_SPEED: f32 = 1.0;
/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Speed range accepted by the Speech API.
pub const MIN_SPEED: f32 = 0.25;
/// Speed range accepted by the Speech API.
pub const MAX_SPEED: f32 = 4.0;
/// Voices advertised via `tts_spec`; the API validates them per request.
pub const SUPPORTED_VOICES: &[&str] = &["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

/// Settings for the `OpenAI Speech API` TTS provider.
///
/// `api_key` is deliberately not part of the struct: it is resolved per
/// request from the host-delivered blob, the request config, or the
/// `OPENAI_API_KEY` environment variable (see [`resolve_api_key`]) so the
/// secret never round-trips through a typed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct OpenAiTtsConfig {
    /// Speech synthesis model (`tts-1` / `tts-1-hd`).
    pub model: String,
    /// Default voice; a per-request voice overrides it.
    pub voice: String,
    /// Speech speed multiplier (0.25–4.0).
    pub speed: f32,
    /// API base URL override (defaults to `https://api.openai.com/v1`).
    pub base_url: String,
}

impl Default for OpenAiTtsConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            speed: DEFAULT_SPEED,
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
    /// has the wrong type, or `speed` is outside the API's 0.25–4.0 range.
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

/// Resolves the effective API key: the host-delivered blob
/// (`plugins.list.openai-tts.config`, via `set_config`) wins, then the
/// per-request config, then the `OPENAI_API_KEY` environment variable.
///
/// # Errors
///
/// Returns a provider error when no key is configured anywhere.
pub fn resolve_api_key(host_config: Option<&Value>, config: &Value) -> Result<String, PluginError> {
    if let Some(key) = host_config
        .and_then(|cfg| cfg.get("api_key"))
        .and_then(resolve_key_value)
    {
        return Ok(key);
    }
    if let Some(key) = config.get("api_key").and_then(resolve_key_value) {
        return Ok(key);
    }
    std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            PluginError::provider(
                "no API key found: set plugins.list.openai-tts.config.api_key \
                 or OPENAI_API_KEY",
            )
        })
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

/// Reads an `api_key` value in either plain-string or
/// `{source: inline|env|auto}` descriptor form.
fn resolve_key_value(value: &Value) -> Option<String> {
    match value {
        Value::String(key) if !key.trim().is_empty() => Some(key.trim().to_string()),
        Value::Object(obj) => {
            let source = obj.get("source").and_then(Value::as_str).unwrap_or("auto");
            match source {
                "inline" => obj
                    .get("inline")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_string),
                "env" => {
                    let var_name = obj
                        .get("env")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("OPENAI_API_KEY");
                    std::env::var(var_name).ok().filter(|key| !key.is_empty())
                }
                // "auto" (or an unrecognized source) falls through to the
                // caller's process-env fallback.
                _ => None,
            }
        }
        _ => None,
    }
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
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn parses_all_fields_and_ignores_unknown() {
        let cfg = OpenAiTtsConfig::from_value(&json!({
            "model": "tts-1-hd",
            "voice": "nova",
            "speed": 1.5,
            "base_url": "https://example.com/v1",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(cfg.model, "tts-1-hd");
        assert_eq!(cfg.voice, "nova");
        assert!((cfg.speed - 1.5).abs() < 1e-6);
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
