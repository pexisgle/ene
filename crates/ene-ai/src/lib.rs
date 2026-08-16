//! # ene-ai
//!
//! Core LLM and embedding provider layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! and the host-facing [`ProviderHost`] lookup contract. Concrete provider
//! backends ship as plugins (`plugins/provider/*`) and are registered in the
//! plugin host; this crate owns only the traits, configuration routing,
//! failover policy, retry policy, and model fetching.
//!
//! Local inference runs in the `ene-plugin-llama-cpp` provider plugin
//! (GGUF/llama.cpp) and [`ene-voice`] (STT/TTS/VAD).
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
/// Effective context-window computation: reconciling the provider-advertised
/// and user-configured windows, then reserving headroom for the response and
/// for token-estimation error.
pub mod context_window;
/// Blanket async-provider adapters over `ene-infer::EngineHandle`
/// (`LocalLlmEngine`, `LocalTtsEngine`, `LocalSttEngine`), plus
/// `EngineDescriptor` capability/concurrency/resource declarations and the
/// process-wide `ResourceRegistry` admission budget.
pub mod engine_adapter;
/// Error types for the AI provider layer.
pub mod error;
/// Message and streaming chunk types (`LlmMessage`, `LlmResponseChunk`, etc.).
pub mod message;
/// Shared, safe model-file downloader (`ModelFetcher`) used by
/// the local-llm plugin (GGUF) and `ene-voice` (Kokoro ONNX / `voices.bin`):
/// in-flight coalescing, `.part` + atomic rename, RAII partial cleanup,
/// HTTPS-only enforcement, pluggable post-download validation, and progress
/// reporting.
pub mod model_fetch;
/// Provider-specific settings relocated into the `plugins.list.<name>`
/// sections (llama.cpp mmproj/acceleration, ONNX dylib path, Kokoro profiles).
pub mod plugin_config;
/// Provider resolution from configuration.
pub mod resolve;
/// Retry policy for transient provider errors.
pub mod retry;
/// Conversation role enum (User, Assistant, System, Tool).
pub mod role;
/// Chat provider routing: task kinds to registry-backed provider instances.
pub mod routing;
/// Provider trait definitions (`LlmProvider`, `EmbeddingProvider`, etc.).
pub mod traits;

pub use config::{
    AiConfig, AiProviderDef, AiTasksConfig, ApiKeyConfig, BUILTIN_PROVIDER_KINDS,
    DEFAULT_LOCAL_ENGINE, FallbackConfig, GpuLayers, LEGACY_OPENAI_COMPATIBLE_KIND,
    LOCAL_ENGINE_CHOICES, LOCAL_PROVIDER, LocalModelDef, OPENAI_PROVIDER_KIND,
    ProactiveAcceleration, RetryConfig, SttConfig, TaskRef, TtsConfig, VadConfig,
    canonical_provider_kind, is_builtin_kind, kind_typo_suggestion,
};
pub use context_window::{
    DEFAULT_CONTEXT_WINDOW, DEFAULT_SAFETY_MARGIN_FRACTION, EffectiveWindow, effective_window,
};
pub use engine_adapter::{
    Capability, CapabilitySet, ConcurrencyHint, DEFAULT_CHUNK_BUFFER, EngineDescriptor, EngineId,
    LlmChatRequest, LlmChatResponse, LocalLlmEngine, LocalSttEngine, LocalTtsEngine,
    ResourceBudgets, ResourceClass, ResourceRegistry, StreamingLocalLlmEngine,
    SttTranscribeRequest, TtsSynthesisRequest, TtsSynthesisResponse,
};
pub use error::{AiError, LlmProviderError};
pub use message::{
    LlmCompletion, LlmMessage, LlmResponseChunk, LlmToolCall, LlmToolCallChunk, UserMessagePart,
};
pub use model_fetch::{
    MagicBytesValidator, ModelFetchError, ModelFetcher, ModelValidator, PrefixPredicateValidator,
    SizeMultipleValidator, sanitize_basename, strip_url_path, validate_https_url,
};
pub use resolve::{
    ChatCandidate, ContextBudgetIssue, FailoverSelection, FallbackRecord, ProviderHealthMonitor,
    ProviderHealthReport, ProviderHealthStatus, ResolvedChat, ResolvedEmbedding,
    ResolvedLocalModel, ResolvedStt, ResolvedTaskRef, ResolvedTts, ResolvedVad, SettingsIssue,
    fetch_model_ids, needs_onboarding, probe_chat_candidates, probe_provider_health,
    resolve_base_url, select_healthy_chat, validate_api_key, validate_context_budgets,
    validate_provider_kinds, validate_settings, warn_on_context_budget_issues,
};
pub use retry::RetryPolicy;
pub use role::Role;
pub use routing::{AiTaskKind, create_chat_provider_for_task, create_task_chat_provider};
pub use traits::{
    AudioProviderError, EmbeddingError, EmbeddingKind, EmbeddingProvider, EmbeddingProviderFactory,
    LlmProvider, LlmProviderFactory, ProviderHost, SttProvider, SttProviderFactory, SttResult,
    TtsChunk, TtsProvider, TtsProviderFactory, VadEngine, VadEvent, VadFactory, cosine_similarity,
    embed, embed_query,
};

/// Token usage accounting for LLM responses.
///
/// Re-exported from `ene-plugin-proto` (the wire-ABI crate every provider
/// layer depends on) so in-process providers, the plugin IPC bridge, and the
/// wire format all share one definition rather than converting between two.
pub use ene_plugin_proto::TokenUsage;
