use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::client;
use crate::config::VoicevoxConfig;
use crate::engine::{EngineProcess, ensure_engine};

/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written.
#[derive(TtsPlugin)]
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
    /// Provider config from `set_config` (handshake / live `SetConfig`),
    /// used by dynamic option listing so the settings UI never starts the
    /// engine just to fetch speakers, and as the canonical config for
    /// synthesis (the request blob may predate an artifact injection).
    config: std::sync::Mutex<Option<VoicevoxConfig>>,
}

impl Default for VoicevoxPlugin {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
            config: std::sync::Mutex::new(None),
        }
    }
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
    fn set_config(&self, config: &Value) {
        if let Ok(config) = VoicevoxConfig::from_value(config.clone()) {
            let mut stored = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let launch_changed = stored
                .as_ref()
                .is_none_or(|previous| previous.launch_key() != config.launch_key());
            *stored = Some(config);
            // A changed launch signature must stop the old engine child:
            // `ensure_engine` keys on the signature and would otherwise
            // reuse a binary started with the previous path/arguments.
            if launch_changed
                && let Some(mut process) = self
                    .engine
                    .try_lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
            {
                // Request termination synchronously (set_config cannot
                // await), then keep the stale child in the mutex:
                // `ensure_engine` reaps it before deciding whether to
                // spawn, so a dying old HTTP endpoint can never satisfy
                // the health probe and skip the restart.
                process.start_kill();
                if let Ok(mut guard) = self.engine.try_lock() {
                    *guard = Some(process);
                }
            }
        }
    }

    fn supports_list_config_options(&self) -> bool {
        true
    }

    fn list_config_options(
        &self,
        path: &str,
    ) -> futures::future::BoxFuture<'_, Vec<ene_plugin::ConfigOption>> {
        if path != "speakers" {
            return Box::pin(async { Vec::new() });
        }
        let config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let config = config.unwrap_or_default();
        Box::pin(async move {
            match crate::client::fetch_speakers(&config).await {
                Ok(speakers) => speakers
                    .into_iter()
                    .flat_map(|speaker| {
                        speaker
                            .styles
                            .into_iter()
                            .map(move |style| ene_plugin::ConfigOption {
                                value: serde_json::json!(style.id),
                                label: format!("{} / {}", speaker.name, style.name),
                                group: Some(speaker.name.clone()),
                            })
                    })
                    .collect(),
                // A down engine (or managed mode with nothing running) has no
                // speaker list; the form keeps the typed field editable.
                Err(_) => Vec::new(),
            }
        })
    }

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
                    "x-ene-ui": { "group": "engine", "order": 0, "impact": "runtime_reload", "label_key": "provider-voicevox-server-url-label", "description_key": "provider-voicevox-server-url-desc" }
                },
                "mode": {
                    "type": "string",
                    "enum": ["external", "managed"],
                    "default": "external",
                    "description": "Engine mode: external uses a running engine; managed spawns server_path when the engine is down",
                    "x-ene-ui": { "group": "engine", "order": 1, "impact": "runtime_reload", "label_key": "provider-voicevox-mode-label", "description_key": "provider-voicevox-mode-desc" }
                },
                "server_path": {
                    "type": "string",
                    "description": "Engine executable path used by managed mode (host-injected from the artifact catalog when installed)",
                    "x-ene-ui": { "group": "engine", "order": 2, "impact": "runtime_reload", "label_key": "provider-voicevox-server-path-label", "description_key": "provider-voicevox-server-path-desc" }
                },
                "server_args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "default": [],
                    "description": "Extra command-line arguments passed to the engine binary in managed mode",
                    "x-ene-ui": { "group": "engine", "order": 3, "impact": "runtime_reload", "label_key": "provider-voicevox-server-args-label", "description_key": "provider-voicevox-server-args-desc" }
                },
                "speaker_id": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Default speaker / style ID (u64; Aivis style IDs exceed u32)",
                    "x-ene-ui": { "group": "voice", "order": 0, "options_path": "speakers", "impact": "runtime_reload", "label_key": "provider-voicevox-speaker-id-label", "description_key": "provider-voicevox-speaker-id-desc" }
                },
                "speed_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Speech speed multiplier (engine-validated, e.g. 0.5-2.0)",
                    "x-ene-ui": { "group": "voice", "order": 1, "impact": "runtime_reload", "slider": { "min": 0.5, "max": 2.0, "step": 0.1 }, "label_key": "provider-voicevox-speed-scale-label", "description_key": "provider-voicevox-speed-scale-desc" }
                },
                "pitch_scale": {
                    "type": "number",
                    "default": 0.0,
                    "description": "Pitch shift (engine-validated, e.g. -0.15-0.15)",
                    "x-ene-ui": { "group": "voice", "order": 2, "impact": "runtime_reload", "slider": { "min": -0.15, "max": 0.15, "step": 0.01 }, "label_key": "provider-voicevox-pitch-scale-label", "description_key": "provider-voicevox-pitch-scale-desc" }
                },
                "intonation_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Intonation strength (engine-validated, e.g. 0-2)",
                    "x-ene-ui": { "group": "voice", "order": 3, "impact": "runtime_reload", "slider": { "min": 0.0, "max": 2.0, "step": 0.1 }, "label_key": "provider-voicevox-intonation-scale-label", "description_key": "provider-voicevox-intonation-scale-desc" }
                },
                "volume_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Output volume (engine-validated, e.g. 0-2)",
                    "x-ene-ui": { "group": "voice", "order": 4, "impact": "runtime_reload", "slider": { "min": 0.0, "max": 2.0, "step": 0.1 }, "label_key": "provider-voicevox-volume-scale-label", "description_key": "provider-voicevox-volume-scale-desc" }
                },
                "tempo_dynamics_scale": {
                    "type": "number",
                    "default": 1.0,
                    "description": "Aivis Speech extension: tempo dynamics strength (0-2); ignored by plain VOICEVOX",
                    "x-ene-ui": { "group": "voice", "order": 5, "impact": "runtime_reload", "slider": { "min": 0.0, "max": 2.0, "step": 0.1 }, "label_key": "provider-voicevox-tempo-dynamics-label", "description_key": "provider-voicevox-tempo-dynamics-desc" }
                },
                "output_sampling_rate": {
                    "type": "integer",
                    "description": "Output sample rate (e.g. 24000/48000); engine default when omitted",
                    "x-ene-ui": { "group": "voice", "order": 6, "impact": "runtime_reload", "label_key": "provider-voicevox-sample-rate-label", "description_key": "provider-voicevox-sample-rate-desc" }
                },
                "startup_timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 10,
                    "description": "How long managed mode waits for GET /version after spawning",
                    "x-ene-ui": { "group": "engine", "order": 4, "impact": "runtime_reload", "label_key": "provider-voicevox-startup-timeout-label", "description_key": "provider-voicevox-startup-timeout-desc" }
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
        // The delivered `set_config` blob is canonical: it carries any
        // artifact-injected `server_path`, which the request blob (raw
        // persisted config) may predate.
        let config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or(VoicevoxConfig::from_value(config)?);
        if config.mode() == crate::config::EngineMode::Managed {
            ensure_engine(&self.engine, &config).await?;
        }
        let speaker = config.resolve_speaker(&voice);
        client::synthesize(&config, &text, speaker).await
    }
}
