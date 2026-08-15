//! Plugin configuration (`plugins.list.edge-tts.config`).

use serde::{Deserialize, Serialize};

use crate::error::EdgeError;

/// Production WebSocket endpoint without query parameters; the client
/// appends `TrustedClientToken`, `ConnectionId`, and `Sec-MS-GEC` per
/// connection.
pub const DEFAULT_ENDPOINT_URL: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";

/// Upper bound on `max_retries`; keeps the worst-case backoff wait bounded
/// even when the setting is misconfigured.
pub const MAX_RETRIES_LIMIT: u32 = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct EdgeTtsConfig {
    /// Edge voice name, short (`ja-JP-NanamiNeural`) or long form.
    pub voice: String,
    /// SSML `xml:lang` value on the `<speak>` element.
    pub locale: String,
    /// Prosody rate adjustment (e.g. `+0%`, `+10%`, `-10%`).
    pub rate: String,
    /// Prosody pitch adjustment (e.g. `+0Hz`, `+10Hz`, `-5Hz`).
    pub pitch: String,
    /// Prosody volume adjustment (e.g. `+0%`, `+10%`).
    pub volume: String,
    /// Reconnect attempts per synthesize request (shared across text
    /// chunks), with exponential backoff.
    pub max_retries: u32,
    /// WebSocket endpoint; must not carry a query string.
    pub endpoint_url: String,
}

impl Default for EdgeTtsConfig {
    fn default() -> Self {
        Self {
            voice: "ja-JP-NanamiNeural".to_string(),
            locale: "ja-JP".to_string(),
            rate: "+0%".to_string(),
            pitch: "+0Hz".to_string(),
            volume: "+0%".to_string(),
            max_retries: 3,
            endpoint_url: DEFAULT_ENDPOINT_URL.to_string(),
        }
    }
}

impl EdgeTtsConfig {
    /// Parses the provider config blob delivered with a synthesize request.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::Config`] when the blob is not an object, a field
    /// has the wrong type, or a prosody value does not match the format the
    /// service accepts.
    pub fn from_value(value: serde_json::Value) -> Result<Self, EdgeError> {
        let config: Self = serde_json::from_value(value)
            .map_err(|e| EdgeError::Config(format!("invalid provider config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Applies a per-request voice override (`ai.tts.voice`).
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::Config`] when the override would break the SSML
    /// voice attribute.
    pub fn with_voice(mut self, voice: &str) -> Result<Self, EdgeError> {
        let voice = voice.trim();
        if !voice.is_empty() {
            if !is_ssml_attribute_safe(voice) {
                return Err(EdgeError::Config(format!(
                    "voice must not contain SSML attribute-breaking characters, got {voice:?}"
                )));
            }
            self.voice = voice.to_string();
        }
        Ok(self)
    }

    fn validate(&self) -> Result<(), EdgeError> {
        if self.voice.trim().is_empty() {
            return Err(EdgeError::Config("voice must not be empty".to_string()));
        }
        if self.locale.trim().is_empty() {
            return Err(EdgeError::Config("locale must not be empty".to_string()));
        }
        if !is_ssml_attribute_safe(&self.voice) {
            return Err(EdgeError::Config(format!(
                "voice must not contain SSML attribute-breaking characters, got {:?}",
                self.voice
            )));
        }
        if !is_ssml_attribute_safe(&self.locale) {
            return Err(EdgeError::Config(format!(
                "locale must not contain SSML attribute-breaking characters, got {:?}",
                self.locale
            )));
        }
        if !is_signed_number(&self.rate, "%") {
            return Err(EdgeError::Config(format!(
                "rate must look like +0% / -10% / +10%, got {:?}",
                self.rate
            )));
        }
        if !is_signed_number(&self.volume, "%") {
            return Err(EdgeError::Config(format!(
                "volume must look like +0% / -10% / +10%, got {:?}",
                self.volume
            )));
        }
        if !is_signed_number(&self.pitch, "Hz") {
            return Err(EdgeError::Config(format!(
                "pitch must look like +0Hz / -5Hz / +10Hz, got {:?}",
                self.pitch
            )));
        }
        if self.max_retries > MAX_RETRIES_LIMIT {
            return Err(EdgeError::Config(format!(
                "max_retries must be at most {MAX_RETRIES_LIMIT}, got {}",
                self.max_retries
            )));
        }
        if self.endpoint_url.trim().is_empty() || self.endpoint_url.contains('?') {
            return Err(EdgeError::Config(
                "endpoint_url must be a wss URL without a query string".to_string(),
            ));
        }
        Ok(())
    }
}

/// Voice names and locales are interpolated into single-quoted SSML
/// attributes; characters that terminate the attribute or open markup would
/// break the document, so they are rejected at config load.
fn is_ssml_attribute_safe(value: &str) -> bool {
    !value
        .chars()
        .any(|c| matches!(c, '\'' | '"' | '&' | '<' | '>'))
}

/// Matches the `[+-]\d+(%|Hz)` shapes the service accepts for prosody.
fn is_signed_number(value: &str, suffix: &str) -> bool {
    let Some(digits) = value
        .strip_prefix(['+', '-'])
        .and_then(|rest| rest.strip_suffix(suffix))
    else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
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
        let config = EdgeTtsConfig::from_value(json!({})).expect("empty config parses");
        assert_eq!(config.voice, "ja-JP-NanamiNeural");
        assert_eq!(config.locale, "ja-JP");
        assert_eq!(config.rate, "+0%");
        assert_eq!(config.pitch, "+0Hz");
        assert_eq!(config.volume, "+0%");
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.endpoint_url, DEFAULT_ENDPOINT_URL);
    }

    #[test]
    fn parses_fields_and_ignores_unknown() {
        let config = EdgeTtsConfig::from_value(json!({
            "voice": "en-US-AvaMultilingualNeural",
            "locale": "en-US",
            "rate": "+10%",
            "pitch": "-5Hz",
            "volume": "+20%",
            "max_retries": 5,
            "endpoint_url": "wss://example.test/edge",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(config.voice, "en-US-AvaMultilingualNeural");
        assert_eq!(config.rate, "+10%");
        assert_eq!(config.pitch, "-5Hz");
        assert_eq!(config.volume, "+20%");
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn rejects_wrong_field_types() {
        let err = EdgeTtsConfig::from_value(json!({"max_retries": "many"}))
            .expect_err("wrong type rejected");
        assert!(err.to_string().contains("configuration"));
    }

    #[test]
    fn rejects_malformed_prosody_values() {
        for key in ["rate", "volume"] {
            let err = EdgeTtsConfig::from_value(json!({ key: "10%" }))
                .expect_err("unsigned value rejected");
            assert!(err.to_string().contains(key));
        }
        let err = EdgeTtsConfig::from_value(json!({"pitch": "+10%"})).expect_err("wrong unit");
        assert!(err.to_string().contains("pitch"));
        let err =
            EdgeTtsConfig::from_value(json!({"rate": "+1.5%"})).expect_err("decimal rejected");
        assert!(err.to_string().contains("rate"));
    }

    #[test]
    fn rejects_oversized_retry_budget() {
        let err = EdgeTtsConfig::from_value(json!({"max_retries": 11})).expect_err("over limit");
        assert!(err.to_string().contains("max_retries"));
    }

    #[test]
    fn voice_override_wins_when_non_empty() {
        let config = EdgeTtsConfig::default();
        assert_eq!(
            config
                .clone()
                .with_voice("en-US-GuyNeural")
                .expect("safe")
                .voice,
            "en-US-GuyNeural"
        );
        assert_eq!(
            config.clone().with_voice("  ").expect("empty").voice,
            "ja-JP-NanamiNeural"
        );
        assert_eq!(
            config.with_voice("").expect("empty").voice,
            "ja-JP-NanamiNeural"
        );
    }

    #[test]
    fn rejects_ssml_unsafe_voice_and_locale() {
        let err = EdgeTtsConfig::from_value(json!({"voice": "ja-JP-O'Neural"}))
            .expect_err("unsafe voice");
        assert!(err.to_string().contains("voice"));
        let err = EdgeTtsConfig::from_value(json!({"locale": "ja<JP"})).expect_err("unsafe locale");
        assert!(err.to_string().contains("locale"));
        let err = EdgeTtsConfig::default()
            .with_voice("x' onmouseover='alert(1)")
            .expect_err("unsafe override");
        assert!(err.to_string().contains("voice"));
    }
}
