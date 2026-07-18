//! # ene-ai
//!
//! Unified LLM and embedding provider layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, the built-in OpenAI-compatible implementation,
//! and local GGUF embedding via Candle.
#![warn(missing_docs)]
#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred locally"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "unit/integration tests use unwrap/expect for assertions"
    )
)]

/// Configuration types for providers and embedding.
pub mod config;
/// Local GGUF quantized embedding provider.
pub mod embedding;
/// Typed `LlmProviderError` enum returned at the library boundary.
pub mod error;
/// Hybrid HyDE / rerank helpers (primary embedder + optional LLM).
pub mod hybrid;
/// Local `llama-server` decision provider (#165).
pub mod local_llm;
/// Unified chat message and streaming types.
pub mod message;
/// Built-in OpenAI-compatible provider and cloud embedding provider.
pub mod openai;
/// Provider traits and registry.
pub mod traits;

pub mod role;

pub use config::{
    ApiKeyConfig, CloudEmbeddingConfig, EmbeddingConfig, LocalEmbeddingConfig,
    ProactiveAcceleration, ProactiveDecisionBackend, ProactiveDecisionFallback,
    ProactiveDecisionProviderConfig, ProactiveProviderConfig, ProviderConfig,
};
pub use embedding::{
    EneEmbeddingError, GgufEmbeddingProvider, create_local_provider, resolve_gguf_paths,
};
pub use error::{AiError, LlmProviderError};
pub use hybrid::{HybridRerankProvider, hyde_document, rerank_tool_specs};
pub use local_llm::{
    DecisionProviderKind, DisabledDecisionProvider, LlamaServerHandle, LlamaServerSpec,
    LocalLlamaCppProvider, ProactiveLlmHandles, build_proactive_llm_handles,
    resolve_llama_server_executable,
};
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
