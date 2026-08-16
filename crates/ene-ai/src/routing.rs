//! Chat provider routing: task kinds to concrete provider instances.
//!
//! Every cloud provider backend ships as a plugin, so provider creation
//! always resolves through the plugin host's [`ProviderHost`]; this module
//! only translates task configuration into a host lookup. The legacy
//! `openai_compatible` kind is folded onto the `openai` plugin kind here.

use crate::config::{AiConfig, TaskRef, canonical_provider_kind};
use crate::error::LlmProviderError;
use crate::traits::{LlmProvider, ProviderHost};

/// Cognitive task kinds that map to [`AiConfig::tasks`] entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTaskKind {
    Chat,
    /// Post-turn affect classifier.
    Classifier,
    /// Proactive speech generation.
    Proactive,
}

/// The [`TaskRef`] backing a cognitive task kind (classifier and proactive
/// fall back to the chat task when unconfigured).
fn task_ref_for_kind(ai: &AiConfig, kind: AiTaskKind) -> &TaskRef {
    match kind {
        AiTaskKind::Chat => &ai.tasks.chat,
        AiTaskKind::Classifier => ai.tasks.classifier.as_ref().unwrap_or(&ai.tasks.chat),
        AiTaskKind::Proactive => ai.tasks.proactive.as_ref().unwrap_or(&ai.tasks.chat),
    }
}

/// Build a chat provider for a named cognitive task.
///
/// The task's own model / `max_tokens` overrides are forwarded so the
/// provider honors task-specific config instead of the `tasks.chat`
/// defaults. Local providers are rejected (chat requires a cloud backend).
pub async fn create_task_chat_provider(
    config: &ene_config::EneConfig,
    kind: AiTaskKind,
    host: &dyn ProviderHost,
) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
    let ai = config
        .get_section::<AiConfig>()
        .map_err(|e| LlmProviderError::Provider(format!("Failed to parse AI config: {e}")))?;
    create_chat_provider_for_task(config, task_ref_for_kind(&ai, kind), host).await
}

/// Build a chat provider for an explicit task reference.
///
/// Resolves the task's provider definition and routes through the global
/// [`ProviderHost`] by kind (with the legacy `openai_compatible` alias
/// folded onto the `openai` plugin kind), so plugin-provided backends
/// (`OpenAI`, Anthropic) all resolve the same way. A local provider is
/// rejected — chat workloads require a cloud backend.
pub async fn create_chat_provider_for_task(
    config: &ene_config::EneConfig,
    task: &TaskRef,
    host: &dyn ProviderHost,
) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
    let ai = config
        .get_section::<AiConfig>()
        .map_err(|e| LlmProviderError::Provider(format!("Failed to parse AI config: {e}")))?;

    if AiConfig::is_local_provider(&task.provider) {
        return Err(LlmProviderError::Provider(
            "chat tasks cannot use the local provider; configure a cloud provider kind".to_string(),
        ));
    }
    let def = ai.get_provider(&task.provider)?;
    host.create_llm_provider(canonical_provider_kind(&def.kind), config, task)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiProviderDef, ApiKeyConfig};
    use crate::traits::{EmbeddingProvider, ProviderHost};
    use crate::{
        AudioProviderError, EmbeddingError, LlmProviderError, SttProvider, TtsProvider, VadEngine,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    /// Stub host whose LLM factory map is empty, standing in for a plugin
    /// host that serves no providers.
    struct EmptyHost;

    #[async_trait]
    impl ProviderHost for EmptyHost {
        async fn create_llm_provider(
            &self,
            kind: &str,
            _config: &ene_config::EneConfig,
            _task: &TaskRef,
        ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
            Err(LlmProviderError::Provider(format!(
                "No LlmProviderFactory registered for provider kind: '{kind}'"
            )))
        }

        async fn create_embedding_provider(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Arc<dyn EmbeddingProvider>, EmbeddingError> {
            Err(EmbeddingError::Init(
                "stub host serves no embedding providers".to_string(),
            ))
        }

        async fn create_tts_provider(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
            Err(AudioProviderError::Provider(
                "stub host serves no TTS providers".to_string(),
            ))
        }

        async fn create_stt_provider(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn SttProvider>, AudioProviderError> {
            Err(AudioProviderError::Provider(
                "stub host serves no STT providers".to_string(),
            ))
        }

        async fn create_vad_engine(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn VadEngine>, AudioProviderError> {
            Err(AudioProviderError::Provider(
                "stub host serves no VAD engines".to_string(),
            ))
        }
    }

    fn config_with_provider(name: &str, kind: &str) -> ene_config::EneConfig {
        let mut ai = AiConfig::default();
        ai.providers.insert(
            name.to_string(),
            AiProviderDef {
                kind: kind.to_string(),
                api_key: ApiKeyConfig {
                    source: "inline".to_string(),
                    inline: "sk-test".to_string(),
                    env: String::new(),
                },
                ..AiProviderDef::default()
            },
        );
        let mut config = ene_config::EneConfig::default();
        config.set_section(&ai).expect("ai config merges");
        config
    }

    #[test]
    fn canonical_kind_folds_legacy_alias() {
        assert_eq!(canonical_provider_kind("openai_compatible"), "openai");
        assert_eq!(canonical_provider_kind("openai"), "openai");
        assert_eq!(canonical_provider_kind("anthropic"), "anthropic");
    }

    #[test]
    fn task_ref_kinds_fall_back_to_chat() {
        let ai = AiConfig::default();
        assert_eq!(
            task_ref_for_kind(&ai, AiTaskKind::Classifier).provider,
            ai.tasks.chat.provider
        );
        assert_eq!(
            task_ref_for_kind(&ai, AiTaskKind::Proactive).provider,
            ai.tasks.chat.provider
        );
        assert_eq!(
            task_ref_for_kind(&ai, AiTaskKind::Chat).provider,
            ai.tasks.chat.provider
        );
    }

    #[tokio::test]
    async fn unregistered_kind_reports_missing_factory() {
        let config = config_with_provider("custom", "not-a-plugin-kind");
        let task = TaskRef {
            provider: "custom".to_string(),
            ..TaskRef::default()
        };
        // `unwrap_err` needs `Debug` on the boxed provider, which trait
        // objects do not provide; match the error instead.
        let Err(err) = create_chat_provider_for_task(&config, &task, &EmptyHost).await else {
            panic!("expected an error, got a provider")
        };
        assert!(err.to_string().contains("not-a-plugin-kind"), "err: {err}");
    }

    #[tokio::test]
    async fn local_provider_is_rejected() {
        let config = ene_config::EneConfig::default();
        let task = TaskRef {
            provider: crate::config::LOCAL_PROVIDER.to_string(),
            ..TaskRef::default()
        };
        assert!(
            create_chat_provider_for_task(&config, &task, &EmptyHost)
                .await
                .is_err()
        );
    }
}
