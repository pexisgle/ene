//! TTS provider factory backed by a plugin IPC connection.
//!
//! [`IpcTtsProviderFactory`] implements [`ene_ai::TtsProviderFactory`] so
//! that plugin-provided TTS providers integrate with the global
//! [`AudioProviderRegistry`](ene_ai::AudioProviderRegistry), mirroring how
//! [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory)
//! integrates LLM providers. The factory is keyed by the plugin's
//! `TtsProviderSpec.kind`, which is also the name users select via
//! `ai.tts.provider`.

use std::sync::Arc;

use ene_ai::AudioProviderError;
use ene_ai::traits::{TtsProvider, TtsProviderFactory};
use ene_plugin_proto::ConcurrencyHint;

use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::ConcurrencyLimiter;
use crate::ipc_tts::IpcTtsProvider;

/// Audio format the host adapter can decode into PCM. Plugin TTS engines
/// (VOICEVOX, Aivis Speech, Kokoro) all emit WAV; the wire contract carries
/// the format echo so a future provider that only emits another container
/// can be added with a matching decoder.
const TTS_AUDIO_FORMAT: &str = "wav";

/// Factory that creates [`IpcTtsProvider`] instances for a specific
/// provider kind served by a plugin binary.
pub struct IpcTtsProviderFactory {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    /// Shared across every provider instance this factory creates; a single
    /// long-lived provider is built per process, but proactive and
    /// interactive turns may still synthesize concurrently, so the declared
    /// [`ConcurrencyHint`] must bound them jointly.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcTtsProviderFactory {
    /// Creates a new factory for the given provider kind, sharing the
    /// plugin connection.
    ///
    /// `concurrency` is the [`ConcurrencyHint`] the plugin declared for this
    /// provider kind during the handshake (or the safe serial default if it
    /// declared none).
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        concurrency: ConcurrencyHint,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            limiter: Arc::new(ConcurrencyLimiter::new(concurrency)),
        }
    }
}

impl TtsProviderFactory for IpcTtsProviderFactory {
    fn provider_name(&self) -> &str {
        &self.kind
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
        let blob = ene_ai::plugin_config::plugin_config_blob(config, &self.plugin_name)
            .unwrap_or_default();
        let voice = config
            .get_section::<ene_ai::AiConfig>()
            .ok()
            .and_then(|ai| ai.resolve_tts())
            .filter(|resolved| resolved.provider == self.kind)
            .and_then(|resolved| resolved.voice);
        Ok(Box::new(IpcTtsProvider::new(
            self.kind.clone(),
            Arc::clone(&self.conn),
            self.plugin_name.clone(),
            voice,
            TTS_AUDIO_FORMAT.to_string(),
            blob,
            Arc::clone(&self.limiter),
        )))
    }
}
