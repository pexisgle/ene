//! # ene-ai
//!
//! Unified LLM and embedding provider layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, the built-in OpenAI-compatible implementation,
//! and local GGUF embedding via Candle.
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Configuration types for providers and embedding.
pub mod config;
/// Local GGUF quantized embedding provider.
pub mod embedding;
/// Typed `LlmProviderError` enum returned at the library boundary.
pub mod error;
/// Hybrid HyDE / rerank helpers (primary embedder + optional LLM).
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
pub use embedding::{
    EneEmbeddingError, GgufEmbeddingProvider, create_local_provider, resolve_gguf_paths,
};
pub use error::{AiError, LlmProviderError};
pub use hybrid::{HybridRerankProvider, hyde_document, rerank_tool_specs};
pub use message::{LlmMessage, LlmResponseChunk, LlmToolCall, LlmToolCallChunk, UserMessagePart};
pub use openai::{
    CloudEmbeddingProvider, OpenAiProvider, OpenAiProviderFactory,
    create_openai_compatible_chat_provider,
};
pub use role::Role;
pub use traits::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmProvider, LlmProviderFactory,
    LlmProviderRegistry, collect_chat_completion, cosine_similarity, embed, embed_query,
};
