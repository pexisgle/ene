//! Local GGUF model providers for the ene AI character platform.
//!
//! Provides in-process llama.cpp inference (decision/chat), GGUF embedding,
//! and automatic GGUF weight download/caching.
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
        clippy::panic,
        reason = "unit/integration tests use unwrap/expect/panic for assertions"
    )
)]

/// Local GGUF quantized embedding provider.
pub mod embedding;
/// GGUF download and path resolution.
pub mod gguf;
/// Shared llama-cpp-2 adapter (decision + embedding).
pub(crate) mod llama_cpp;
/// In-process llama.cpp decision provider.
pub mod local_llm;

pub use embedding::{EneEmbeddingError, GgufEmbeddingProvider, create_local_provider};
pub use gguf::{
    ensure_gguf_available, ensure_mmproj_available, prefetch_configured_gguf,
    prefetch_decision_gguf, prefetch_embedding_gguf, resolve_decision_gguf_path,
    resolve_embedding_gguf_path, resolve_local_gguf_path,
};
pub use local_llm::{
    DecisionProviderKind, DisabledDecisionProvider, LocalGgufLoadParams, LocalLlamaCppProvider,
    ProactiveLlmHandles, build_proactive_llm_handles,
};
