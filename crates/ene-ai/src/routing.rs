//! Chat provider routing: task kinds to concrete provider instances.
//!
//! Every cloud provider backend ships as a plugin, so provider creation
//! always resolves through the global [`LlmProviderRegistry`]; this module
//! only translates task configuration into a registry lookup. The legacy
//! `openai_compatible` kind is folded onto the `openai` plugin kind here.

use crate::config::{AiConfig, TaskRef, canonical_provider_kind};
use crate::error::LlmProviderError;
use crate::traits::{LlmProvider, LlmProviderRegistry};

/// Cognitive task kinds that map to [`AiConfig::tasks`] entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTaskKind {
    /// Main conversation chat.
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
pub fn create_task_chat_provider(
    config: &ene_config::EneConfig,
    kind: AiTaskKind,
) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
    let ai = config
        .get_section::<AiConfig>()
        .map_err(|e| LlmProviderError::Provider(format!("Failed to parse AI config: {e}")))?;
    create_chat_provider_for_task(config, task_ref_for_kind(&ai, kind))
}

/// Build a chat provider for an explicit task reference.
///
/// Resolves the task's provider definition and routes through the global
/// [`LlmProviderRegistry`] by kind (with the legacy `openai_compatible`
/// alias folded onto the `openai` plugin kind), so plugin-provided backends
/// (`OpenAI`, Anthropic) all resolve the same way. A local provider is
/// rejected — chat workloads require a cloud backend.
pub fn create_chat_provider_for_task(
    config: &ene_config::EneConfig,
    task: &TaskRef,
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
    LlmProviderRegistry::create_provider(canonical_provider_kind(&def.kind), config, task)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise assertions"
)]
mod tests {
    use super::*;
    use crate::config::{AiProviderDef, ApiKeyConfig};

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

    #[test]
    fn unregistered_kind_reports_missing_factory() {
        let config = config_with_provider("custom", "not-a-plugin-kind");
        let mut task = TaskRef::default();
        task.provider = "custom".to_string();
        // `unwrap_err` needs `Debug` on the boxed provider, which trait
        // objects do not provide; match the error instead.
        let err = match create_chat_provider_for_task(&config, &task) {
            Ok(_) => panic!("expected an error, got a provider"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("not-a-plugin-kind"), "err: {err}");
    }

    #[test]
    fn local_provider_is_rejected() {
        let config = ene_config::EneConfig::default();
        let mut task = TaskRef::default();
        task.provider = crate::config::LOCAL_PROVIDER.to_string();
        assert!(create_chat_provider_for_task(&config, &task).is_err());
    }
}
