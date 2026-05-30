//! # ene-provider
//!
//! Unified LLM provider abstraction layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, and the built-in OpenAI-compatible implementation.
#![warn(missing_docs)]

/// Unified chat message and streaming types.
pub mod message;
/// Configuration types for providers and embedding.
pub mod config;
/// Provider traits and registry.
pub mod traits;
/// Built-in OpenAI-compatible provider and cloud embedding provider.
pub mod openai;

pub use message::{
    LlmMessage, LlmResponseChunk, LlmToolCall, LlmToolCallChunk, UserMessagePart,
};
pub use config::{LocalEmbeddingConfig, ProviderConfig};
pub use traits::{EmbeddingProvider, LlmProvider, LlmProviderFactory, LlmProviderRegistry};
pub use openai::{CloudEmbeddingProvider, OpenAiProvider, OpenAiProviderFactory};
