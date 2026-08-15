use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default `ElevenLabs` model (multilingual, low latency).
pub const DEFAULT_MODEL: &str = "eleven_multilingual_v2";
/// Default output sample rate; the WAV header and the API's `pcm_{rate}`
/// format must agree, so both are driven by the same setting.
pub const DEFAULT_SAMPLE_RATE: u32 = 24_000;
/// PCM sample rates the API offers. The API rejects other `pcm_*` rates,
/// so the config is validated against this set rather than clamped.
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[16_000, 24_000, 44_100];
pub const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io/v1";
/// Character limit of the `/stream` endpoint (the lowest across
/// `ElevenLabs` models).
pub const MAX_INPUT_CHARS: usize = 5_000;
/// Default environment variable consulted when no base URL is configured.
pub const BASE_URL_ENV: &str = "ELEVENLABS_BASE_URL";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct VoiceSettings {
    /// Stability (0.0–1.0; higher = more consistent but flatter).
    pub stability: f32,
    /// Similarity to the original voice (0.0–1.0).
    pub similarity_boost: f32,
    /// Style exaggeration (0.0–1.0; `style` is only supported by some models).
    pub style: f32,
    pub use_speaker_boost: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            stability: 0.5,
            similarity_boost: 0.75,
            style: 0.0,
            use_speaker_boost: true,
        }
    }
}

impl VoiceSettings {
    /// Clamps each float into the API's 0.0–1.0 range. Non-finite values
    /// (which `clamp` silently passes through) are rejected by
    /// [`ElevenLabsConfig::from_value`] before this is reachable.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            stability: self.stability.clamp(0.0, 1.0),
            similarity_boost: self.similarity_boost.clamp(0.0, 1.0),
            style: self.style.clamp(0.0, 1.0),
            use_speaker_boost: self.use_speaker_boost,
        }
    }
}

/// Settings for the `ElevenLabs` TTS provider.
///
/// `api_key` is deliberately not part of the struct: the host resolves it
/// into the credential store and injects it at request time, so the secret
/// never round-trips through the plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ElevenLabsConfig {
    pub model_id: String,
    /// Default voice ID; a per-request voice overrides it. `ElevenLabs`
    /// voices are user-specific, so there is no closed list to validate
    /// against — only shape.
    pub voice_id: String,
    /// Output sample rate; selects the API's `pcm_{rate}` format and the
    /// WAV header rate.
    pub sample_rate: u32,
    /// Voice synthesis settings sent with every request.
    pub voice_settings: VoiceSettings,
    /// API base URL override (defaults to `https://api.elevenlabs.io/v1`).
    pub base_url: String,
}

impl Default for ElevenLabsConfig {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_MODEL.to_string(),
            voice_id: String::new(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            voice_settings: VoiceSettings::default(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }
}

impl ElevenLabsConfig {
    /// Parses the provider config blob delivered with a synthesize request.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the blob is not a JSON object, a field
    /// has the wrong type, a float is non-finite, `sample_rate` is outside
    /// the supported PCM set, `model_id` is empty, or `voice_id` is not a
    /// safe URL path segment.
    pub fn from_value(value: &Value) -> Result<Self, PluginError> {
        let mut config: Self = serde_json::from_value(value.clone()).map_err(|e| {
            PluginError::provider(format!("invalid elevenlabs provider config: {e}"))
        })?;
        if config.model_id.trim().is_empty() {
            return Err(PluginError::provider(
                "invalid elevenlabs provider config: model_id must not be empty",
            ));
        }
        if !config.voice_id.trim().is_empty() {
            validate_voice_id(&config.voice_id)?;
        }
        if !SUPPORTED_SAMPLE_RATES.contains(&config.sample_rate) {
            return Err(PluginError::provider(format!(
                "invalid elevenlabs provider config: sample_rate {} is not one of \
                 {SUPPORTED_SAMPLE_RATES:?}",
                config.sample_rate
            )));
        }
        let settings = config.voice_settings;
        // JSON cannot express NaN/Infinity (serde_json maps them to `null`),
        // so this only guards Values assembled outside `serde_json`; `clamp`
        // would silently pass non-finite floats through.
        for (name, value) in [
            ("stability", settings.stability),
            ("similarity_boost", settings.similarity_boost),
            ("style", settings.style),
        ] {
            if !value.is_finite() {
                return Err(PluginError::provider(format!(
                    "invalid elevenlabs provider config: voice_settings.{name} must be finite"
                )));
            }
        }
        config.voice_settings = settings.clamped();
        config.model_id = config.model_id.trim().to_string();
        config.voice_id = config.voice_id.trim().to_string();
        Ok(config)
    }

    /// Resolves the voice ID for a request: a non-empty request voice wins,
    /// then the configured `voice_id`.
    #[must_use]
    pub fn resolve_voice(&self, request_voice: &str) -> Option<String> {
        let candidate = if request_voice.trim().is_empty() {
            self.voice_id.as_str()
        } else {
            request_voice.trim()
        };
        (!candidate.is_empty()).then(|| candidate.to_string())
    }
}

/// Rejects voice IDs that could escape the URL path segment they are
/// interpolated into.
///
/// # Errors
///
/// Returns a provider error when the ID is empty or contains any character
/// outside `[A-Za-z0-9_-]` (`ElevenLabs` IDs are generated from this set).
pub fn validate_voice_id(voice_id: &str) -> Result<(), PluginError> {
    let trimmed = voice_id.trim();
    if trimmed.is_empty() {
        return Err(PluginError::provider(
            "invalid elevenlabs provider config: voice_id must not be empty",
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(PluginError::provider(
            "invalid elevenlabs provider config: voice_id may only contain \
             ASCII letters, digits, '-' and '_'",
        ));
    }
    Ok(())
}

/// Resolves the effective API base URL with the same precedence as the key:
/// host blob, request config, `ELEVENLABS_BASE_URL`, then the API default.
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
                std::env::var(BASE_URL_ENV)
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
        let cfg = ElevenLabsConfig::from_value(&json!({})).expect("empty config parses");
        assert_eq!(cfg.model_id, DEFAULT_MODEL);
        assert!(cfg.voice_id.is_empty());
        assert_eq!(cfg.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(cfg.voice_settings, VoiceSettings::default());
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn parses_all_fields_and_ignores_unknown() {
        let cfg = ElevenLabsConfig::from_value(&json!({
            "model_id": "eleven_turbo_v2_5",
            "voice_id": "21m00Tcm4TlvDq8ikWAM",
            "sample_rate": 44_100,
            "voice_settings": {
                "stability": 0.2,
                "similarity_boost": 0.9,
                "style": 0.1,
                "use_speaker_boost": false
            },
            "base_url": "https://example.com/v1",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(cfg.model_id, "eleven_turbo_v2_5");
        assert_eq!(cfg.voice_id, "21m00Tcm4TlvDq8ikWAM");
        assert_eq!(cfg.sample_rate, 44_100);
        assert!((cfg.voice_settings.stability - 0.2).abs() < 1e-6);
        assert!((cfg.voice_settings.similarity_boost - 0.9).abs() < 1e-6);
        assert!((cfg.voice_settings.style - 0.1).abs() < 1e-6);
        assert!(!cfg.voice_settings.use_speaker_boost);
        assert_eq!(cfg.base_url, "https://example.com/v1");
    }

    #[test]
    fn rejects_wrong_field_types() {
        let err = ElevenLabsConfig::from_value(&json!({"sample_rate": "fast"}))
            .expect_err("wrong type rejected");
        assert!(err.to_string().contains("provider"));
    }

    #[test]
    fn rejects_unsupported_sample_rates() {
        for rate in [0, 8_000, 22_050, 48_000] {
            let err = ElevenLabsConfig::from_value(&json!({"sample_rate": rate}))
                .expect_err("unsupported rate rejected");
            assert!(err.to_string().contains("sample_rate"), "{err}");
        }
    }

    #[test]
    fn accepts_all_supported_sample_rates() {
        for rate in SUPPORTED_SAMPLE_RATES {
            let cfg = ElevenLabsConfig::from_value(&json!({"sample_rate": rate}))
                .expect("supported rate accepted");
            assert_eq!(cfg.sample_rate, *rate);
        }
    }

    #[test]
    fn clamps_out_of_range_voice_settings() {
        let cfg = ElevenLabsConfig::from_value(&json!({
            "voice_settings": {
                "stability": 1.5,
                "similarity_boost": -0.5,
                "style": 2.0,
                "use_speaker_boost": true
            }
        }))
        .expect("clampable settings parse");
        assert!((cfg.voice_settings.stability - 1.0).abs() < 1e-6);
        assert!(cfg.voice_settings.similarity_boost.abs() < 1e-6);
        assert!((cfg.voice_settings.style - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_empty_model_id() {
        let err = ElevenLabsConfig::from_value(&json!({"model_id": "  "}))
            .expect_err("empty model rejected");
        assert!(err.to_string().contains("model_id"));
    }

    #[test]
    fn rejects_unsafe_voice_ids() {
        for voice in ["a/b", "a?b", "a#b", "a b", "../x"] {
            let err = ElevenLabsConfig::from_value(&json!({"voice_id": voice}))
                .expect_err("unsafe voice rejected");
            assert!(err.to_string().contains("voice_id"), "{voice}: {err}");
        }
    }

    #[test]
    fn accepts_valid_voice_ids() {
        for voice in ["21m00Tcm4TlvDq8ikWAM", "Rachel", "my-voice_2"] {
            let cfg = ElevenLabsConfig::from_value(&json!({"voice_id": voice}))
                .expect("valid voice accepted");
            assert_eq!(cfg.voice_id, voice);
        }
    }

    #[test]
    fn empty_voice_id_is_allowed_in_config_when_request_voice_carries_it() {
        let cfg = ElevenLabsConfig::from_value(&json!({})).expect("empty voice parses");
        assert_eq!(cfg.resolve_voice("Rachel").as_deref(), Some("Rachel"));
        assert_eq!(cfg.resolve_voice("  "), None);
    }

    #[test]
    fn request_voice_wins_then_config() {
        let cfg = ElevenLabsConfig::from_value(&json!({"voice_id": "config-voice"}))
            .expect("config parses");
        assert_eq!(
            cfg.resolve_voice("request-voice").as_deref(),
            Some("request-voice")
        );
        assert_eq!(cfg.resolve_voice("").as_deref(), Some("config-voice"));
        assert_eq!(cfg.resolve_voice("   ").as_deref(), Some("config-voice"));
    }

    #[test]
    fn validates_voice_ids() {
        assert!(validate_voice_id("21m00Tcm4TlvDq8ikWAM").is_ok());
        assert!(validate_voice_id("").is_err());
        assert!(validate_voice_id("a/b").is_err());
    }
}
