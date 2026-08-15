//! Edge-TTS plugin: capabilities and synthesis handler.

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::audio::{decode_mp3, encode_wav};
use crate::client;
use crate::config::EdgeTtsConfig;
use crate::ssml::{chunk_text, escape_xml, sanitize};

/// TTS plugin serving Microsoft Edge Neural Voice over the free, keyless
/// WebSocket endpoint.
///
/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written.
#[derive(TtsPlugin, Default)]
#[provider(
    kind = "edge-tts",
    formats = "wav",
    // Stateless cloud service: every synthesize call opens its own
    // WebSocket connection, so parallel requests cannot interfere.
    concurrency = 2,
    queue_depth = 4,
)]
pub struct EdgeTtsPlugin;

impl ene_plugin::ConfigurablePlugin for EdgeTtsPlugin {
    /// Captures the broker socket/token so every connection is
    /// host-mediated.
    fn set_sandbox(&self, sandbox: &ene_plugin_proto::SandboxConfigData) {
        crate::broker::configure_broker(sandbox);
    }

    /// Advertises the settings surface for `plugins.list.edge-tts.config`.
    /// No API keys are involved: the service is the anonymous Edge Read
    /// Aloud endpoint.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "voice": {
                    "type": "string",
                    "default": "ja-JP-NanamiNeural",
                    "description": "Edge voice name, short (ja-JP-NanamiNeural) or long form",
                    "x-ene-ui": { "group": "voice", "order": 0, "options_path": "voices", "impact": "runtime_reload" }
                },
                "locale": {
                    "type": "string",
                    "default": "ja-JP",
                    "description": "SSML xml:lang value on the <speak> element",
                    "x-ene-ui": { "group": "voice", "order": 1, "impact": "runtime_reload" }
                },
                "rate": {
                    "type": "string",
                    "default": "+0%",
                    "description": "Prosody rate adjustment (e.g. +10%, -10%)",
                    "x-ene-ui": { "group": "voice", "order": 2, "impact": "runtime_reload" }
                },
                "pitch": {
                    "type": "string",
                    "default": "+0Hz",
                    "description": "Prosody pitch adjustment (e.g. +5Hz, -5Hz)",
                    "x-ene-ui": { "group": "voice", "order": 3, "impact": "runtime_reload" }
                },
                "volume": {
                    "type": "string",
                    "default": "+0%",
                    "description": "Prosody volume adjustment (e.g. +10%, -10%)",
                    "x-ene-ui": { "group": "voice", "order": 4, "impact": "runtime_reload" }
                },
                "max_retries": {
                    "type": "integer",
                    "x-ene-ui": { "group": "voice", "order": 5, "advanced": true, "impact": "runtime_reload" },
                    "minimum": 0,
                    "maximum": 10,
                    "default": 3,
                    "description": "Reconnect attempts per synthesize request (shared across chunks) with exponential backoff"
                },
                "endpoint_url": {
                    "type": "string",
                    "default": "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1",
                    "description": "WebSocket endpoint; must not carry a query string"
                }
            }
        }))
    }
}

#[async_trait]
impl TtsPlugin for EdgeTtsPlugin {
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
                "edge-tts only emits wav audio; requested format: {format}"
            )));
        }
        let config = EdgeTtsConfig::from_value(config)?.with_voice(&voice)?;
        let chunks = chunk_text(&escape_xml(&sanitize(&text)));
        if chunks.is_empty() {
            return Err(PluginError::provider(
                "edge-tts: no speakable text after sanitization",
            ));
        }
        let mp3 = client::synthesize(&config, &chunks).await?;
        let decoded = decode_mp3(&mp3)?;
        Ok(encode_wav(&decoded.pcm, decoded.sample_rate)?)
    }
}
