//! Shared llama-cpp-4 adapter for local decision + embedding (#171).
//!
//! Keeps low-level llama.cpp types inside `ene-ai-local`; callers use typed helpers only.

mod backend;
mod embed;
mod generate;
mod grammar;
mod load;

pub(crate) use backend::with_backend;
pub(crate) use embed::embed_text;
pub(crate) use generate::{generate_chat, generate_vision};
pub(crate) use load::{LoadSpec, LoadedModel, resource_class_for};

use ene_ai::error::LlmProviderError;

pub(crate) fn map_llama_err(context: &str, err: impl std::fmt::Display) -> LlmProviderError {
    LlmProviderError::LocalLlm(format!("{context}: {err}"))
}
