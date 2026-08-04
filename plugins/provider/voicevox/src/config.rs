//! Plugin configuration (`plugins.list.voicevox.config`).

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};

/// Default VOICEVOX engine HTTP endpoint.
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:50021";

/// Settings for the VOICEVOX-compatible TTS provider.
///
/// Field names are the `snake_case` keys documented in
/// `docs/configuration.md`; the wire API (`AudioQuery` JSON) uses `camelCase`
/// and is handled separately in [`crate::client`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct VoicevoxConfig {
    /// Engine HTTP base URL (VOICEVOX: 50021, Aivis Speech: 10101).
    pub server_url: String,
    /// Default speaker / style ID. Aivis Speech style IDs exceed `u32`, so
    /// the field is 64-bit.
    pub speaker_id: u64,
    /// Speech speed multiplier (engine-validated range, default 1.0).
    pub speed_scale: f32,
    /// Pitch shift (engine-validated range, default 0.0).
    pub pitch_scale: f32,
    /// Intonation strength (engine-validated range, default 1.0).
    pub intonation_scale: f32,
    /// Output volume (engine-validated range, default 1.0).
    pub volume_scale: f32,
    /// Aivis Speech extension: tempo dynamics strength (0.0–2.0, default
    /// 1.0). Only sent when non-default, because VOICEVOX engines reject
    /// unknown `AudioQuery` fields.
    pub tempo_dynamics_scale: f32,
    /// Output sample rate (e.g. 24000 / 48000). Only sent when set; the
    /// engine default (24000 for VOICEVOX) applies otherwise.
    pub output_sampling_rate: Option<u32>,
    /// Whether to spawn the engine binary when the server is not running.
    pub auto_start: bool,
    /// Engine executable path used by managed mode.
    pub engine_path: Option<String>,
    /// Extra command-line arguments passed to the engine binary.
    pub engine_args: Vec<String>,
    /// How long managed mode waits for `GET /version` to succeed after
    /// spawning the engine.
    pub startup_timeout_secs: u64,
}

impl Default for VoicevoxConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            speaker_id: 0,
            speed_scale: 1.0,
            pitch_scale: 0.0,
            intonation_scale: 1.0,
            volume_scale: 1.0,
            tempo_dynamics_scale: 1.0,
            output_sampling_rate: None,
            auto_start: false,
            engine_path: None,
            engine_args: Vec::new(),
            startup_timeout_secs: 10,
        }
    }
}

impl VoicevoxConfig {
    /// Parses the provider config blob delivered with a synthesize request.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the blob is not a JSON object or a
    /// field has the wrong type.
    pub fn from_value(value: serde_json::Value) -> Result<Self, PluginError> {
        serde_json::from_value(value)
            .map_err(|e| PluginError::provider(format!("invalid voicevox provider config: {e}")))
    }

    /// Resolves the speaker for a request: a non-empty `voice` value that
    /// parses as an integer overrides the configured default speaker.
    #[must_use]
    pub fn resolve_speaker(&self, voice: &str) -> u64 {
        voice.parse::<u64>().unwrap_or(self.speaker_id)
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = VoicevoxConfig::from_value(json!({})).expect("empty config parses");
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
        assert_eq!(cfg.speaker_id, 0);
        assert!((cfg.speed_scale - 1.0).abs() < 1e-4);
        assert!(!cfg.auto_start);
        assert!(cfg.engine_path.is_none());
        assert_eq!(cfg.startup_timeout_secs, 10);
    }

    #[test]
    fn parses_snake_case_fields_and_ignores_unknown() {
        let cfg = VoicevoxConfig::from_value(json!({
            "server_url": "http://127.0.0.1:10101",
            "speaker_id": 42,
            "speed_scale": 1.5,
            "pitch_scale": 0.05,
            "intonation_scale": 0.8,
            "volume_scale": 0.9,
            "tempo_dynamics_scale": 1.2,
            "output_sampling_rate": 48000,
            "auto_start": true,
            "engine_path": "/opt/voicevox/run",
            "engine_args": ["--port", "10101"],
            "startup_timeout_secs": 30,
            "voice": "14",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(cfg.server_url, "http://127.0.0.1:10101");
        assert_eq!(cfg.speaker_id, 42);
        assert!((cfg.speed_scale - 1.5).abs() < 1e-4);
        assert_eq!(cfg.output_sampling_rate, Some(48_000));
        assert!(cfg.auto_start);
        assert_eq!(cfg.engine_path.as_deref(), Some("/opt/voicevox/run"));
        assert_eq!(cfg.engine_args, vec!["--port", "10101"]);
        assert_eq!(cfg.startup_timeout_secs, 30);
    }

    #[test]
    fn rejects_wrong_field_types() {
        let err = VoicevoxConfig::from_value(json!({"speaker_id": "not-a-number"}))
            .expect_err("wrong type rejected");
        assert!(err.to_string().contains("provider"));
    }

    #[test]
    fn voice_override_wins_when_numeric() {
        let cfg = VoicevoxConfig::from_value(json!({"speaker_id": 7})).expect("config parses");
        assert_eq!(cfg.resolve_speaker("42"), 42);
        assert_eq!(cfg.resolve_speaker("not-a-speaker"), 7);
        assert_eq!(cfg.resolve_speaker(""), 7);
    }
}
