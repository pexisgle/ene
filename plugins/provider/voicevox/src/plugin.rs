//! VOICEVOX-compatible TTS plugin: capabilities and synthesis handler.

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::client;
use crate::config::VoicevoxConfig;
use crate::engine::{EngineProcess, ensure_engine};

/// TTS plugin serving the VOICEVOX / Aivis Speech HTTP API.
///
/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written.
#[derive(TtsPlugin, Default)]
#[provider(
    kind = "voicevox",
    formats = "wav",
    // A local engine answers one synthesis at a time; the serial default is
    // declared explicitly, per the ConcurrencyHint design.
    concurrency = 1,
    queue_depth = 2,
)]
pub struct VoicevoxPlugin {
    /// Managed-mode engine child, spawned lazily on first use so the
    /// handshake stays fast. `Drop` kills it (see [`EngineProcess`]).
    pub(crate) engine: Mutex<Option<EngineProcess>>,
}

impl Drop for VoicevoxPlugin {
    fn drop(&mut self) {
        // `try_lock` rather than `lock`: Drop runs during runtime teardown,
        // where a synthesize task could still hold the guard. The server
        // joins connection tasks before dropping the dispatch, so in
        // practice the lock is free; a contended lock just leaks the engine
        // child to the OS instead of blocking shutdown.
        if let Ok(mut guard) = self.engine.try_lock() {
            drop(guard.take());
        }
    }
}

impl ene_plugin::ConfigurablePlugin for VoicevoxPlugin {
    /// Advertises the settings surface for `plugins.list.voicevox.config`.
    /// No API keys are involved: the engine is a local HTTP server.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "server_url": {
                    "type": "string",
                    "default": "http://127.0.0.1:50021",
                    "description": "Engine HTTP base URL (VOICEVOX: 50021, Aivis Speech: 10101)",
                    "x-ene-ui": { "group": "engine", "order": 0, "impact": "runtime_reload" }
                },
                "speaker_id": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Default speaker / style ID (u64; Aivis style IDs exceed u32)",
                    "x-ene-ui": { "group": "voice", "order": 0, "options_path": "speakers", "impact": "runtime_reload" }
                },
                "speed_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Speech speed multiplier (engine-validated, e.g. 0.5-2.0)",
                    "x-ene-ui": { "group": "voice", "order": 1, "impact": "runtime_reload" }
                },
                "pitch_scale": {
                    "type": "number",
                    "default": 0.0,
                    "description": "Pitch shift (engine-validated, e.g. -0.15-0.15)",
                    "x-ene-ui": { "group": "voice", "order": 2, "impact": "runtime_reload" }
                },
                "intonation_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Intonation strength (engine-validated, e.g. 0-2)",
                    "x-ene-ui": { "group": "voice", "order": 3, "impact": "runtime_reload" }
                },
                "volume_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Output volume (engine-validated, e.g. 0-2)"
                },
                "tempo_dynamics_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Aivis Speech extension: tempo dynamics strength (0-2); ignored by plain VOICEVOX"
                },
                "output_sampling_rate": {
                    "type": "integer",
                    "description": "Output sample rate (e.g. 24000/48000); engine default when omitted"
                },
                "auto_start": {
                    "type": "boolean",
                    "default": false,
                    "description": "Spawn the engine binary when the server is not running"
                },
                "engine_path": {
                    "type": "string",
                    "description": "Engine executable path used when auto_start is enabled"
                },
                "engine_args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "default": [],
                    "description": "Extra command-line arguments passed to the engine binary"
                },
                "startup_timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 10,
                    "description": "How long managed mode waits for GET /version after spawning"
                }
            }
        }))
    }
}

#[async_trait]
impl TtsPlugin for VoicevoxPlugin {
    fn tts_capabilities(&self) -> Vec<TtsProviderSpec> {
        vec![Self::tts_spec()]
    }

    async fn synthesize(
        &self,
        kind: &str,
        config: Value,
        text: String,
        voice: String,
        format: String,
    ) -> Result<Vec<u8>, PluginError> {
        if kind != Self::TTS_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        if format != "wav" {
            return Err(PluginError::provider(format!(
                "voicevox only emits wav audio; requested format: {format}"
            )));
        }
        let config = VoicevoxConfig::from_value(config)?;
        if config.auto_start {
            ensure_engine(&self.engine, &config).await?;
        }
        let speaker = config.resolve_speaker(&voice);
        client::synthesize(&config, &text, speaker).await
    }
}
