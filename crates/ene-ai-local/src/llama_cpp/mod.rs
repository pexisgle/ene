//! Shared llama-cpp-2 adapter for local decision + embedding (#171).
//!
//! Keeps low-level llama.cpp types inside `ene-ai`; callers use typed helpers only.

mod backend;
mod embed;
mod generate;
mod load;

pub(crate) use embed::embed_text;
pub(crate) use generate::{generate_chat, generate_with_rgb_image};
pub(crate) use load::{LoadSpec, LoadedModel};

use ene_ai::error::LlmProviderError;

pub(crate) fn map_llama_err(context: &str, err: impl std::fmt::Display) -> LlmProviderError {
    LlmProviderError::LocalLlm(format!("{context}: {err}"))
}
