//! # ene-ai
//!
//! Unified LLM and embedding provider layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, the built-in OpenAI-compatible implementation,
//! and local GGUF embedding / decision inference via llama-cpp-2.
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
/// GGUF download and path resolution (#171).
pub mod gguf;
/// Hybrid HyDE / rerank helpers (primary embedder + optional LLM).
pub mod hybrid;
/// Shared llama-cpp-2 adapter (decision + embedding).
pub(crate) mod llama_cpp;
/// In-process llama.cpp decision provider (#165 / #171).
pub mod local_llm;
/// Unified chat message and streaming types.
pub mod message;
/// Built-in OpenAI-compatible provider and cloud embedding provider.
pub mod openai;
/// Resolved settings from [`config::AiConfig`] task routing.
pub mod resolve;
/// Provider traits and registry.
pub mod traits;

pub mod role;

pub use config::{
    AiConfig, AiProviderDef, AiTasksConfig, ApiKeyConfig, LOCAL_PROVIDER, LocalModelDef,
    ProactiveAcceleration, TaskRef,
};
pub use embedding::{EneEmbeddingError, GgufEmbeddingProvider, create_local_provider};
pub use error::{AiError, LlmProviderError};
pub use gguf::{
    ensure_gguf_available, prefetch_configured_gguf, prefetch_decision_gguf,
    prefetch_embedding_gguf, resolve_decision_gguf_path, resolve_embedding_gguf_path,
    resolve_local_gguf_path,
};
pub use hybrid::{HybridRerankProvider, hyde_document, rerank_tool_specs};
pub use local_llm::{
    DecisionProviderKind, DisabledDecisionProvider, LocalGgufLoadParams, LocalLlamaCppProvider,
    ProactiveLlmHandles, build_proactive_llm_handles,
};
pub use message::{LlmMessage, LlmResponseChunk, LlmToolCall, LlmToolCallChunk, UserMessagePart};
pub use openai::{
    AiTaskKind, CloudEmbeddingProvider, OpenAiProvider, OpenAiProviderFactory,
    create_chat_provider_from_resolved, create_task_chat_provider,
};
pub use resolve::{
    ResolvedChat, ResolvedEmbedding, ResolvedLocalModel, ResolvedTaskRef, resolve_base_url,
};
pub use role::Role;
pub use traits::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmProvider, LlmProviderFactory,
    LlmProviderRegistry, collect_chat_completion, cosine_similarity, embed, embed_query,
};
