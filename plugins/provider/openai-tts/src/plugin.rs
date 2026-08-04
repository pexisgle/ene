//! `OpenAI Speech API` TTS plugin: capabilities and synthesis handler.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::client;
use crate::config::{OpenAiTtsConfig, SUPPORTED_VOICES, resolve_api_key, resolve_base_url};

/// Configuration delivered by the host at handshake time
/// (`plugins.list.openai-tts.config`), stored per process.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
pub(crate) static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// TTS plugin serving the `OpenAI Speech API`.
///
/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written.
#[derive(TtsPlugin)]
#[provider(
    kind = "openai_tts",
    voices = "alloy, echo, fable, onyx, nova, shimmer",
    formats = "wav",
    // A stateless HTTP proxy to a cloud API, not a local model — safe to
    // run many requests concurrently, mirroring the openai plugin's
    // explicit concurrency declaration.
    concurrency = 8,
    queue_depth = 16,
)]
pub struct OpenAiTtsPlugin;

impl ene_plugin::ConfigurablePlugin for OpenAiTtsPlugin {
    /// Receives the plugin configuration blob from the host at handshake
    /// time (`plugins.list.openai-tts.config`).
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
                    "description": "OpenAI API key, or a {source: inline|env|auto} descriptor"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (defaults to https://api.openai.com/v1)"
                },
                "model": {
                    "type": "string",
                    "enum": ["tts-1", "tts-1-hd"],
                    "default": "tts-1",
                    "description": "Speech synthesis model (tts-1 for low latency, tts-1-hd for higher quality)"
                },
                "voice": {
                    "type": "string",
                    "enum": SUPPORTED_VOICES,
                    "default": "alloy",
                    "description": "Default voice; a per-request voice overrides it"
                },
                "speed": {
                    "type": "number",
                    "minimum": 0.25,
                    "maximum": 4.0,
                    "default": 1.0,
                    "description": "Speech speed multiplier (0.25-4.0)"
                }
            }
        }))
    }
}

#[async_trait]
impl TtsPlugin for OpenAiTtsPlugin {
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
                "openai-tts only emits wav audio; requested format: {format}"
            )));
        }
        if text.trim().is_empty() {
            return Err(PluginError::provider("cannot synthesize empty text"));
        }

        let parsed = OpenAiTtsConfig::from_value(&config)?;
        let host_config = PLUGIN_CONFIG
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let api_key = resolve_api_key(host_config.as_ref(), &config)?;
        let base_url = resolve_base_url(host_config.as_ref(), &config);
        let voice = parsed.resolve_voice(&voice);
        client::synthesize(&parsed, &api_key, &base_url, &text, &voice).await
    }
}
