//! Live provider catalog: routes provider creation to the current plugin host.
//!
//! Wraps the shared plugin-host slot so callers hold one
//! `Arc<dyn ene_ai::ProviderHost>` regardless of host restarts: each call
//! locks the slot and delegates to the manager's [`ene_ai::ProviderHost`]
//! implementation. The slot starts empty (the plugin host starts after the
//! embedder, which needs the catalog for lazy resolution) and is filled by
//! bootstrap; reconfiguration swaps the manager inside it.

use std::sync::Arc;

use ene_plugin_host::PluginHostManager;
use tokio::sync::Mutex;

/// Shared slot holding the current plugin host manager, if any.
///
/// `None` while the host has not started yet or after startup failed; the
/// actor swaps the manager in place during plugin reconfiguration.
pub(crate) type PluginHostSlot = Arc<Mutex<Option<PluginHostManager>>>;

/// [`ene_ai::ProviderHost`] implementation backed by a [`PluginHostSlot`].
#[derive(Clone)]
pub(crate) struct LiveProviderCatalog {
    slot: PluginHostSlot,
}

impl LiveProviderCatalog {
    /// Creates a catalog delegating to whatever manager the slot holds.
    #[must_use]
    pub(crate) fn new(slot: PluginHostSlot) -> Self {
        Self { slot }
    }
}

fn host_unavailable(kind: &str) -> ene_ai::LlmProviderError {
    ene_ai::LlmProviderError::Provider(format!(
        "plugin host is not running; cannot create provider kind '{kind}'"
    ))
}

#[async_trait::async_trait]
impl ene_ai::ProviderHost for LiveProviderCatalog {
    async fn create_llm_provider(
        &self,
        kind: &str,
        config: &ene_config::EneConfig,
        task: &ene_ai::TaskRef,
    ) -> Result<Box<dyn ene_ai::LlmProvider>, ene_ai::LlmProviderError> {
        let guard = self.slot.lock().await;
        let host = guard.as_ref().ok_or_else(|| host_unavailable(kind))?;
        ene_ai::ProviderHost::create_llm_provider(host, kind, config, task).await
    }

    async fn create_embedding_provider(
        &self,
        kind: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
        let guard = self.slot.lock().await;
        let host = guard.as_ref().ok_or_else(|| {
            ene_ai::EmbeddingError::Init(format!(
                "plugin host is not running; cannot create embedding provider kind '{kind}'"
            ))
        })?;
        ene_ai::ProviderHost::create_embedding_provider(host, kind, config).await
    }

    async fn create_tts_provider(
        &self,
        kind: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn ene_ai::TtsProvider>, ene_ai::AudioProviderError> {
        let guard = self.slot.lock().await;
        let host = guard.as_ref().ok_or_else(|| {
            ene_ai::AudioProviderError::Provider(format!(
                "plugin host is not running; cannot create TTS provider kind '{kind}'"
            ))
        })?;
        ene_ai::ProviderHost::create_tts_provider(host, kind, config).await
    }

    async fn create_stt_provider(
        &self,
        kind: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn ene_ai::SttProvider>, ene_ai::AudioProviderError> {
        let guard = self.slot.lock().await;
        let host = guard.as_ref().ok_or_else(|| {
            ene_ai::AudioProviderError::Provider(format!(
                "plugin host is not running; cannot create STT provider kind '{kind}'"
            ))
        })?;
        ene_ai::ProviderHost::create_stt_provider(host, kind, config).await
    }

    async fn create_vad_engine(
        &self,
        kind: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn ene_ai::VadEngine>, ene_ai::AudioProviderError> {
        let guard = self.slot.lock().await;
        let host = guard.as_ref().ok_or_else(|| {
            ene_ai::AudioProviderError::Provider(format!(
                "plugin host is not running; cannot create VAD engine kind '{kind}'"
            ))
        })?;
        ene_ai::ProviderHost::create_vad_engine(host, kind, config).await
    }
}
