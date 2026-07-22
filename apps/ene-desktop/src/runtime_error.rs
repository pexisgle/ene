//! User-facing runtime error messages mapped to localized Fluent strings (#242).

use ene_runtime::EneRuntimeError;

/// Map a bootstrap / open failure into a localized user message.
pub fn user_message(error: &EneRuntimeError) -> String {
    match error {
        EneRuntimeError::NoCharacterCard => crate::i18n::runtime_error_no_character_card(),
        EneRuntimeError::ChannelClosed => crate::i18n::runtime_error_channel_closed(),
        EneRuntimeError::MindPrerequisite(name) => {
            crate::i18n::runtime_error_mind_prerequisite((*name).to_string())
        }
        EneRuntimeError::Bootstrap(msg) => crate::i18n::runtime_error_bootstrap(msg.clone()),
        EneRuntimeError::Config(err) => crate::i18n::runtime_error_config(err.to_string()),
        EneRuntimeError::Memory(err) => crate::i18n::runtime_error_memory(err.to_string()),
        EneRuntimeError::Mind(err) => crate::i18n::runtime_error_mind(err.to_string()),
        EneRuntimeError::Tool(err) => crate::i18n::runtime_error_tool(err.to_string()),
        EneRuntimeError::Ai(err) => user_message_from_ai(err),
    }
}

/// Map a turn-terminal failure payload into a localized user message.
pub fn user_message_from_turn(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return crate::i18n::runtime_error_turn_failed_unknown();
    }
    crate::i18n::runtime_error_turn_failed(trimmed.to_string())
}

fn user_message_from_ai(error: &ene_ai::AiError) -> String {
    match error {
        ene_ai::AiError::Llm(err) => match err {
            ene_ai::LlmProviderError::Auth(_) => crate::i18n::runtime_error_ai_auth(),
            ene_ai::LlmProviderError::RateLimit(_) => crate::i18n::runtime_error_ai_rate_limit(),
            ene_ai::LlmProviderError::Network(_) => crate::i18n::runtime_error_ai_network(),
            ene_ai::LlmProviderError::LocalLlm(_) => {
                crate::i18n::runtime_error_ai_local_llm(err.to_string())
            }
            ene_ai::LlmProviderError::ContentFilter(_)
            | ene_ai::LlmProviderError::Truncated { .. }
            | ene_ai::LlmProviderError::Provider(_) => {
                crate::i18n::runtime_error_ai_provider(err.to_string())
            }
        },
        ene_ai::AiError::Embedding(err) => crate::i18n::runtime_error_ai_embedding(err.to_string()),
        ene_ai::AiError::MissingApiKey(_) => crate::i18n::runtime_error_ai_auth(),
        ene_ai::AiError::MissingBaseUrl(provider) => {
            crate::i18n::runtime_error_config(format!("missing base URL for {provider}"))
        }
        ene_ai::AiError::InvalidBaseUrl(url) => {
            crate::i18n::runtime_error_config(format!("invalid base URL: {url}"))
        }
    }
}
