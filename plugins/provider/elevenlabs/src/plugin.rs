//! `ElevenLabs` TTS plugin: capabilities and synthesis handler.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::client;
use crate::config::{
    DEFAULT_MODEL, DEFAULT_SAMPLE_RATE, ElevenLabsConfig, MAX_INPUT_CHARS, SUPPORTED_SAMPLE_RATES,
    resolve_base_url, validate_voice_id,
};

/// TTS plugin serving the `ElevenLabs` API.
///
/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written.
#[derive(Default, TtsPlugin)]
#[provider(
    kind = "elevenlabs",
    formats = "wav",
    // A stateless cloud proxy, not a local model — safe to run many
    // requests concurrently, mirroring the openai plugin's explicit
    // concurrency declaration.
    concurrency = 8,
    queue_depth = 16,
)]
pub struct ElevenLabsPlugin {
    config: Mutex<Option<Value>>,
}

impl ElevenLabsPlugin {
    fn delivered_config(&self) -> Option<Value> {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ene_plugin::ConfigurablePlugin for ElevenLabsPlugin {
    /// Receives the plugin configuration blob from the host at handshake
    /// time (`plugins.list.elevenlabs.config`).
    fn set_config(&self, config: &Value) {
        *self.config.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    }

    /// Captures the broker socket/token so every request is host-mediated.
    fn set_sandbox(&self, sandbox: &ene_plugin_proto::SandboxConfigData) {
        crate::broker::configure_broker(sandbox);
    }

    fn supports_list_config_options(&self) -> bool {
        true
    }

    fn list_config_options(
        &self,
        path: &str,
    ) -> futures::future::BoxFuture<'_, Vec<ene_plugin::ConfigOption>> {
        if path != "voices" {
            return Box::pin(async { Vec::new() });
        }
        let config = self
            .delivered_config()
            .and_then(|blob| ElevenLabsConfig::from_value(&blob).ok())
            .unwrap_or_default();
        Box::pin(async move {
            crate::client::fetch_voices(&config)
                .await
                .unwrap_or_default()
        })
    }

    /// Advertises the config schema; `api_key` is marked `x-ene-secret: true`
    /// so the host masks/redacts it. The key itself is unused by the plugin:
    /// the host injects it into broker requests by key name.
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
                    "description": "ElevenLabs API key, or a {source: inline|env|auto} descriptor",
                    "x-ene-ui": { "label_key": "provider-elevenlabs-api-key-label", "description_key": "provider-elevenlabs-api-key-desc" }
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (defaults to https://api.elevenlabs.io/v1)",
                    "x-ene-ui": { "group": "connection", "order": 0, "impact": "runtime_reload", "label_key": "provider-elevenlabs-base-url-label", "description_key": "provider-elevenlabs-base-url-desc" }
                },
                "model_id": {
                    "type": "string",
                    "default": DEFAULT_MODEL,
                    "description": "ElevenLabs model ID (e.g. eleven_multilingual_v2)",
                    "x-ene-ui": { "group": "voice", "order": 0, "impact": "runtime_reload", "label_key": "provider-elevenlabs-model-id-label", "description_key": "provider-elevenlabs-model-id-desc" }
                },
                "voice_id": {
                    "type": "string",
                    "description": "Default voice ID; a per-request voice overrides it. Required when the request carries none.",
                    "x-ene-ui": { "group": "voice", "order": 1, "options_path": "voices", "impact": "runtime_reload", "label_key": "provider-elevenlabs-voice-id-label", "description_key": "provider-elevenlabs-voice-id-desc" }
                },
                "sample_rate": {
                    "type": "integer",
                    "enum": SUPPORTED_SAMPLE_RATES,
                    "default": DEFAULT_SAMPLE_RATE,
                    "description": "PCM output sample rate; selects the API's pcm_{rate} format and the WAV header rate",
                    "x-ene-ui": { "group": "voice", "order": 2, "advanced": true, "impact": "runtime_reload", "label_key": "provider-elevenlabs-sample-rate-label", "description_key": "provider-elevenlabs-sample-rate-desc" }
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

        // The delivered `set_config` blob is canonical; the request blob is
        // the fallback when the host never delivered one.
        let delivered = self.delivered_config();
        let parsed = match delivered.as_ref() {
            Some(blob) => ElevenLabsConfig::from_value(blob)?,
            None => ElevenLabsConfig::from_value(&config)?,
        };
        let voice_id = parsed.resolve_voice(&voice).ok_or_else(|| {
            PluginError::provider(
                "no voice selected: set plugins.list.elevenlabs.config.voice_id \
                 or pass a per-request voice",
            )
        })?;
        validate_voice_id(&voice_id)?;
        let base_url = resolve_base_url(delivered.as_ref(), &config);
        client::synthesize_rest(&parsed, &base_url, &text, &voice_id).await
    }
}
