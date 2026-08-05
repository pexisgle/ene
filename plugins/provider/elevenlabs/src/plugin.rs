//! `ElevenLabs` TTS plugin: capabilities and synthesis handler.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::client;
use crate::config::{
    DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, ElevenLabsConfig, MAX_INPUT_CHARS, Mode,
    SUPPORTED_SAMPLE_RATES, resolve_api_key, resolve_base_url, validate_voice_id,
};
use crate::ws;

/// Configuration delivered by the host at handshake time
/// (`plugins.list.elevenlabs.config`), stored per process.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
pub(crate) static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// TTS plugin serving the `ElevenLabs` API.
///
/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written.
#[derive(TtsPlugin)]
#[provider(
    kind = "elevenlabs",
    formats = "wav",
    // A stateless cloud proxy, not a local model — safe to run many
    // requests concurrently, mirroring the openai plugin's explicit
    // concurrency declaration.
    concurrency = 8,
    queue_depth = 16,
)]
pub struct ElevenLabsPlugin;

impl ene_plugin::ConfigurablePlugin for ElevenLabsPlugin {
    /// Receives the plugin configuration blob from the host at handshake
    /// time (`plugins.list.elevenlabs.config`).
    fn set_config(&self, config: &Value) {
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    }

    /// Advertises the config schema; `api_key` is marked `x-ene-secret: true`
    /// so the host masks/redacts it.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "api_key": {
                    "oneOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "properties": {
                                "source": {
                                    "type": "string",
                                    "enum": ["inline", "env", "auto"]
                                },
                                "inline": { "type": "string" },
                                "env": { "type": "string" }
                            }
                        }
                    ],
                    "x-ene-secret": true,
                    "description": "ElevenLabs API key, or a {source: inline|env|auto} descriptor"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (defaults to https://api.elevenlabs.io/v1; websocket mode swaps the scheme)"
                },
                "mode": {
                    "type": "string",
                    "enum": ["rest", "ws"],
                    "default": "rest",
                    "description": "Transport: rest (POST /text-to-speech/{voice_id}/stream) or ws (stream-input websocket)"
                },
                "model_id": {
                    "type": "string",
                    "default": DEFAULT_MODEL,
                    "description": "ElevenLabs model ID (e.g. eleven_multilingual_v2)"
                },
                "voice_id": {
                    "type": "string",
                    "description": "Default voice ID; a per-request voice overrides it. Required when the request carries none."
                },
                "sample_rate": {
                    "type": "integer",
                    "enum": SUPPORTED_SAMPLE_RATES,
                    "default": DEFAULT_SAMPLE_RATE,
                    "description": "PCM output sample rate; selects the API's pcm_{rate} format and the WAV header rate"
                },
                "voice_settings": {
                    "type": "object",
                    "properties": {
                        "stability": {
                            "type": "number",
                            "minimum": 0.0,
                            "maximum": 1.0,
                            "default": 0.5,
                            "description": "Voice stability (clamped to 0.0-1.0)"
                        },
                        "similarity_boost": {
                            "type": "number",
                            "minimum": 0.0,
                            "maximum": 1.0,
                            "default": 0.75,
                            "description": "Voice similarity boost (clamped to 0.0-1.0)"
                        },
                        "style": {
                            "type": "number",
                            "minimum": 0.0,
                            "maximum": 1.0,
                            "default": 0.0,
                            "description": "Style exaggeration (clamped to 0.0-1.0; only some models support it)"
                        },
                        "use_speaker_boost": {
                            "type": "boolean",
                            "default": true,
                            "description": "Whether to boost the voice's natural characteristics"
                        }
                    }
                }
            }
        }))
    }
}

#[async_trait]
impl TtsPlugin for ElevenLabsPlugin {
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
                "elevenlabs only emits wav audio; requested format: {format}"
            )));
        }
        if text.trim().is_empty() {
            return Err(PluginError::provider("cannot synthesize empty text"));
        }
        if text.chars().count() > MAX_INPUT_CHARS {
            return Err(PluginError::provider(format!(
                "input exceeds the ElevenLabs API's {MAX_INPUT_CHARS}-character limit"
            )));
        }

        let parsed = ElevenLabsConfig::from_value(&config)?;
        let voice_id = parsed.resolve_voice(&voice).ok_or_else(|| {
            PluginError::provider(
                "no voice selected: set plugins.list.elevenlabs.config.voice_id \
                 or pass a per-request voice",
            )
        })?;
        validate_voice_id(&voice_id)?;
        let host_config = PLUGIN_CONFIG
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let api_key = resolve_api_key(host_config.as_ref(), &config)?;
        let base_url = resolve_base_url(host_config.as_ref(), &config);
        match parsed.mode {
            Mode::Rest => {
                client::synthesize_rest(&parsed, &api_key, &base_url, &text, &voice_id).await
            }
            Mode::Ws => ws::synthesize_ws(&parsed, &api_key, &base_url, &text, &voice_id).await,
        }
    }
}
