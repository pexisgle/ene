//! Plugin configuration (`plugins.list.voicevox.config`).

use ene_plugin::PluginError;
use serde::{Deserialize, Serialize};

pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:50021";

/// `mode = "external"`: always talk to an engine the user starts and manages
/// themselves (or an existing VOICEVOX / Aivis Speech install).
pub const MODE_EXTERNAL: &str = "external";
/// `mode = "managed"`: spawn `server_path` when the engine is not running.
pub const MODE_MANAGED: &str = "managed";

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
    /// Engine mode: `"external"` (default; use a running engine) or
    /// `"managed"` (spawn `server_path` when the engine is not running).
    pub mode: String,
    /// Engine executable path used by managed mode.
    pub server_path: Option<String>,
    /// Extra command-line arguments passed to the engine binary.
    pub server_args: Vec<String>,
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
            mode: MODE_EXTERNAL.to_string(),
            server_path: None,
            server_args: Vec::new(),
            startup_timeout_secs: 10,
        }
    }
}

/// Effective engine mode after resolving the `mode` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineMode {
    /// Use an already-running engine; never spawn a child.
    External,
    /// Spawn `server_path` when the engine is not running.
    Managed,
}

impl VoicevoxConfig {
    pub fn from_value(value: serde_json::Value) -> Result<Self, PluginError> {
        serde_json::from_value(value)
            .map_err(|e| PluginError::provider(format!("invalid voicevox provider config: {e}")))
    }

    /// Resolves the configured engine mode. Unknown values fall back to
    /// [`EngineMode::External`] with a warning, so a typo cannot silently
    /// spawn a binary.
    #[must_use]
    pub fn mode(&self) -> EngineMode {
        match self.mode.trim() {
            MODE_MANAGED => EngineMode::Managed,
            MODE_EXTERNAL | "" => EngineMode::External,
            other => {
                tracing::warn!(
                    component = "VoicevoxPlugin",
                    mode = other,
                    "unknown voicevox mode; falling back to external"
                );
                EngineMode::External
            }
        }
    }

    /// The launch signature for managed-mode engine restart decisions.
    #[must_use]
    pub fn launch_key(&self) -> crate::engine::LaunchKey {
        crate::engine::LaunchKey::from_config(self)
    }

    /// Resolves the speaker for a request: a non-empty `voice` value that
    /// parses as an integer overrides the configured default speaker.
    /// Unparseable values fall back to the configured speaker and log a
    /// warning, so a typo (or a named voice from another provider) is not
    /// silently swallowed.
    #[must_use]
    pub fn resolve_speaker(&self, voice: &str) -> u64 {
        match voice.parse::<u64>() {
            Ok(speaker) => speaker,
            Err(_) if voice.trim().is_empty() => self.speaker_id,
            Err(_) => {
                tracing::warn!(
                    component = "VoicevoxPlugin",
                    voice,
                    speaker_id = self.speaker_id,
                    "ai.tts.voice is not a numeric speaker id; using the configured speaker_id"
                );
                self.speaker_id
            }
        }
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
        assert_eq!(cfg.mode(), EngineMode::External);
        assert!(cfg.server_path.is_none());
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
            "mode": "managed",
            "server_path": "/opt/voicevox/run",
            "server_args": ["--port", "10101"],
            "startup_timeout_secs": 30,
            "voice": "14",
            "future_key": "preserved"
        }))
        .expect("config parses");
        assert_eq!(cfg.server_url, "http://127.0.0.1:10101");
        assert_eq!(cfg.speaker_id, 42);
        assert!((cfg.speed_scale - 1.5).abs() < 1e-4);
        assert_eq!(cfg.output_sampling_rate, Some(48_000));
        assert_eq!(cfg.mode(), EngineMode::Managed);
        assert_eq!(cfg.server_path.as_deref(), Some("/opt/voicevox/run"));
        assert_eq!(cfg.server_args, vec!["--port", "10101"]);
        assert_eq!(cfg.startup_timeout_secs, 30);
    }

    #[test]
    fn unknown_mode_falls_back_to_external() {
        let cfg = VoicevoxConfig::from_value(json!({"mode": "auto"})).expect("config parses");
        assert_eq!(cfg.mode(), EngineMode::External);
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
