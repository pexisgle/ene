//! STT provider factory backed by a plugin IPC connection.
//!
//! [`IpcSttProviderFactory`] implements [`ene_ai::SttProviderFactory`] so
//! that plugin-provided STT providers integrate with the global
//! [`AudioProviderRegistry`](ene_ai::AudioProviderRegistry), mirroring how
//! [`IpcTtsProviderFactory`](crate::tts_factory::IpcTtsProviderFactory)
//! integrates TTS providers. The factory is keyed by the plugin's
//! `SttProviderSpec.kind`, which is also the name users select via
//! `ai.stt.provider`.

use std::sync::Arc;

use ene_ai::AudioProviderError;
use ene_ai::traits::{SttProvider, SttProviderFactory};
use ene_plugin_proto::ConcurrencyHint;

use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::ConcurrencyLimiter;
use crate::ipc_stt::{IpcSttProvider, STT_AUDIO_FORMAT};

/// Factory that creates [`IpcSttProvider`] instances for a specific
/// provider kind served by a plugin binary.
pub struct IpcSttProviderFactory {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    /// Shared across every provider instance this factory creates; a single
    /// long-lived provider is built per capture session, but sessions may
    /// overlap, so the declared [`ConcurrencyHint`] must bound them jointly.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcSttProviderFactory {
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

impl SttProviderFactory for IpcSttProviderFactory {
    fn provider_name(&self) -> &str {
        &self.kind
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn SttProvider>, AudioProviderError> {
        let blob = ene_ai::plugin_config::plugin_config_blob(config, &self.plugin_name)
            .unwrap_or_default();
        Ok(Box::new(IpcSttProvider::new(
            self.kind.clone(),
            Arc::clone(&self.conn),
            self.plugin_name.clone(),
            STT_AUDIO_FORMAT.to_string(),
            blob,
            Arc::clone(&self.limiter),
        )))
    }
}
