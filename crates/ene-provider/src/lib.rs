//! # ene-provider
//!
//! Unified LLM provider abstraction layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, and the built-in OpenAI-compatible implementation.
#![warn(missing_docs)]

/// Configuration types for providers and embedding.
pub mod config;
/// Typed `LlmProviderError` enum returned at the library boundary.
pub mod error;
/// Hybrid rerank provider (primary embedder + optional LLM for `HyDE` / rerank).
pub mod hybrid;
/// Unified chat message and streaming types.
pub mod message;
/// Built-in OpenAI-compatible provider and cloud embedding provider.
pub mod openai;
/// Provider traits and registry.
pub mod traits;

pub mod role;

pub use config::{
    ApiKeyConfig, CloudEmbeddingConfig, EmbeddingConfig, LocalEmbeddingConfig, ProviderConfig,
};
pub use error::LlmProviderError;
pub use hybrid::HybridRerankProvider;
pub use message::{LlmMessage, LlmResponseChunk, LlmToolCall, LlmToolCallChunk, UserMessagePart};
pub use openai::{CloudEmbeddingProvider, OpenAiProvider, OpenAiProviderFactory};
pub use role::Role;
pub use traits::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmProvider, LlmProviderFactory,
    LlmProviderRegistry, cosine_similarity,
};
