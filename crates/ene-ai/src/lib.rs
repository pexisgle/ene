//! # ene-ai
//!
//! Core LLM and embedding provider layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, and the built-in OpenAI-compatible implementation.
//!
//! Local providers are in [`ene-ai-local`] (GGUF/llama.cpp) and
//! [`ene-voice`] (STT/TTS/VAD).
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

/// Configuration types for AI providers, tasks, and retry policies.
pub mod config;
/// Error types for the AI provider layer.
pub mod error;
/// Provider health monitoring and failover routing.
pub mod health;
/// Message and streaming chunk types (`LlmMessage`, `LlmResponseChunk`, etc.).
pub mod message;
/// OpenAI-compatible provider implementation.
pub mod openai;
/// Provider resolution from configuration.
pub mod resolve;
/// Retry policy for transient provider errors.
pub mod retry;
/// Conversation role enum (User, Assistant, System, Tool).
pub mod role;
/// Provider trait definitions (`LlmProvider`, `EmbeddingProvider`, etc.).
pub mod traits;

pub use config::{
    AiConfig, AiProviderDef, AiTasksConfig, ApiKeyConfig, BUILTIN_PROVIDER_KINDS, FallbackConfig,
    LOCAL_PROVIDER, LocalModelDef, ProactiveAcceleration, RetryConfig, SttConfig, TaskRef,
    TtsConfig, VadConfig, is_builtin_kind, kind_typo_suggestion,
};
pub use error::{AiError, LlmProviderError};
pub use health::{
    FailoverSelection, FallbackRecord, HealthCheckError, ProviderHealthMonitor,
    ProviderHealthReport, ProviderHealthStatus, check_provider_health, select_healthy_chat,
};
pub use message::{LlmMessage, LlmResponseChunk, LlmToolCall, LlmToolCallChunk, UserMessagePart};
pub use openai::{
    AiTaskKind, CloudEmbeddingProvider, OpenAiProvider, OpenAiProviderFactory,
    create_chat_provider_from_resolved, create_task_chat_provider,
};
pub use resolve::{
    ChatCandidate, ResolvedChat, ResolvedEmbedding, ResolvedLocalModel, ResolvedStt,
    ResolvedTaskRef, ResolvedTts, ResolvedVad, SettingsIssue, needs_onboarding, resolve_base_url,
    validate_api_key, validate_provider_kinds, validate_settings,
};
pub use retry::RetryPolicy;
pub use role::Role;
pub use traits::{
    AudioProviderError, AudioProviderRegistry, EmbeddingError, EmbeddingKind, EmbeddingProvider,
    LlmProvider, LlmProviderFactory, LlmProviderRegistry, SttProvider, SttProviderFactory,
    SttResult, TtsChunk, TtsProvider, TtsProviderFactory, VadEngine, VadEvent, VadFactory,
    cosine_similarity, embed, embed_query,
};
