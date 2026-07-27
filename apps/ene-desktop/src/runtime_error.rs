//! User-facing runtime error messages mapped to localized Fluent strings (#242).

use ene_runtime::EneRuntimeError;

/// Map a bootstrap / open failure into a localized user message.
pub fn user_message(error: &EneRuntimeError) -> String {
    match error {
        EneRuntimeError::NoCharacterCard => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-no-character-card")
        }
        EneRuntimeError::ChannelClosed => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-channel-closed")
        }
        EneRuntimeError::MindPrerequisite(name) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-mind-prerequisite",
            name = (*name).to_string()
        ),
        EneRuntimeError::Bootstrap(msg) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-bootstrap",
            message = msg.clone()
        ),
        EneRuntimeError::Config(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-config",
            detail = err.to_string()
        ),
        EneRuntimeError::Memory(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-memory",
            detail = err.to_string()
        ),
        EneRuntimeError::Mind(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-mind",
            detail = err.to_string()
        ),
        EneRuntimeError::Tool(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-tool",
            detail = err.to_string()
        ),
        EneRuntimeError::ToolRag(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-tool",
            detail = err.to_string()
        ),
        EneRuntimeError::Ai(err) => user_message_from_ai(err),
    }
}

/// Map a turn-terminal failure payload into a localized user message.
pub fn user_message_from_turn(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-turn-failed-unknown");
    }
    i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "runtime-error-turn-failed",
        detail = trimmed.to_string()
    )
}

fn user_message_from_ai(error: &ene_ai::AiError) -> String {
    match error {
        ene_ai::AiError::Llm(err) => match err {
            ene_ai::LlmProviderError::Auth(_) => {
                i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-ai-auth")
            }
            ene_ai::LlmProviderError::RateLimit(_) => {
                i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-ai-rate-limit")
            }
            ene_ai::LlmProviderError::Network(_) => {
                i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-ai-network")
            }
            ene_ai::LlmProviderError::LocalLlm(_) => i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "runtime-error-ai-local-llm",
                detail = err.to_string()
            ),
            ene_ai::LlmProviderError::Busy(_) => {
                i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-ai-busy")
            }
            ene_ai::LlmProviderError::ContentFilter(_)
            | ene_ai::LlmProviderError::Truncated { .. }
            | ene_ai::LlmProviderError::Provider(_) => i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "runtime-error-ai-provider",
                detail = err.to_string()
            ),
        },
        ene_ai::AiError::Embedding(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-ai-embedding",
            detail = err.to_string()
        ),
        ene_ai::AiError::MissingApiKey(_) => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-ai-auth")
        }
        ene_ai::AiError::MissingBaseUrl(provider) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-config",
            detail = format!("missing base URL for {provider}")
        ),
        ene_ai::AiError::InvalidBaseUrl(url) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-config",
            detail = format!("invalid base URL: {url}")
        ),
    }
}
