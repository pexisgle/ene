//! Blanket async-provider adapters over [`ene_infer::EngineHandle`].
//!
//! # Why this module exists
//!
//! Four local-inference providers in this workspace (llama.cpp chat, the
//! vision mmproj model, GGUF embeddings, and Kokoro/whisper voice) each
//! hand-wrote their own `async fn` implementation of [`crate::LlmProvider`] /
//! [`crate::TtsProvider`] / [`crate::SttProvider`] and each got concurrency
//! wrong in a different way — `spawn_blocking` under an outer
//! `tokio::time::timeout` that cannot actually cancel the blocking work,
//! `block_in_place` panicking off a non-current-thread runtime, or scratch
//! state (a KV cache, a decoder buffer) taken out from behind a mutex around
//! a `spawn_blocking` call and lost forever if that task is cancelled or
//! panics.
//!
//! [`ene_infer`] fixes the underlying discipline once: implementors write a
//! synchronous [`ene_infer::LocalModel`] and the framework supplies the
//! worker thread, bounded queue, cooperative cancellation, and panic
//! recovery. This module is the other half — the async trait `impl` these
//! providers still need to actually be used as an [`crate::LlmProvider`] /
//! etc., written exactly once here so no future provider author writes
//! `spawn_blocking` again:
//!
//! - [`llm::LocalLlmEngine`] — blanket [`crate::LlmProvider`] for any
//!   [`ene_infer::LocalModel`], wrapping a single completed reply.
//! - [`llm::StreamingLocalLlmEngine`] — sibling blanket [`crate::LlmProvider`]
//!   for any [`ene_infer::StreamingLocalModel`], delivering real
//!   token-by-token chunks through [`ene_infer::EngineHandle::submit_stream`]
//!   instead. See that type's docs for why this had to be a separate type
//!   rather than a second `impl` on [`llm::LocalLlmEngine`] itself.
//! - [`tts::LocalTtsEngine`] — blanket [`crate::TtsProvider`].
//! - [`stt::LocalSttEngine`] — blanket [`crate::SttProvider`].
//! - [`descriptor::EngineDescriptor`] — declared capability, concurrency,
//!   and resource-class metadata, replacing "call it and see if it errors"
//!   (e.g. a runtime error when `tools` is non-empty) with an upfront query.
//! - [`resource::ResourceRegistry`] — one shared admission semaphore per
//!   distinct [`ResourceClass`], so independently-constructed
//!   engines that contend on the same physical resource (most importantly:
//!   the same GPU device) are admission-controlled together instead of each
//!   having a lock that only protects itself. This is the host process's
//!   admission authority: plugin-provided providers reach it from
//!   `ene-plugin-host` as well (see `resource`'s module docs), so a plugin
//!   and a local model on the same device share one budget.
//!
//! # Where this does *not* fully work
//!
//! [`ene_infer::LocalModel::run`] is still a one-shot synchronous call: it
//! hands back exactly one `Response` when it returns, with no channel for
//! partial progress. That remains fine for
//! [`crate::TtsProvider::synthesize_stream`] (see [`tts`]'s docs) and for
//! embeddings — neither wants mid-job incremental delivery through this
//! adapter layer — and it is still what [`llm::LocalLlmEngine`] wraps for a
//! chat model that only implements plain [`ene_infer::LocalModel`].
//!
//! A local chat engine that wants genuine token-by-token delivery implements
//! [`ene_infer::StreamingLocalModel`] — an additive extension of
//! [`ene_infer::LocalModel`] that changes nothing about an existing model.
//! [`ene_infer::EngineHandle::submit_stream`] delivers chunks through a
//! bounded channel with the same cooperative
//! cancellation/deadline/caller-gone handling `submit` has, and
//! [`llm::StreamingLocalLlmEngine`] is the [`crate::LlmProvider`] adapter
//! over it. `crates/ene-ai-local`'s `LlamaChatModel` implements
//! [`ene_infer::StreamingLocalModel`] by pushing each detokenized piece
//! `llama_cpp::generate::sample_tokens`'s existing per-token loop already
//! produces into the sink, and `LocalLlamaCppProvider` wraps it in
//! [`llm::StreamingLocalLlmEngine`] — real streaming end to end, proven
//! against real llama.cpp inference by
//! `crates/ene-ai-local/src/local_llm/mod.rs`'s `smoke_gguf_streaming` (a
//! manual, `--ignored` smoke test; automated coverage runs the same
//! streaming battery this crate's own mock model does, against a
//! `StreamingLocalModel` stand-in shaped like `LlamaChatModel`, in
//! `crates/ene-ai-local/src/local_llm/model.rs`'s `conformance_tests`).
//!
//! What remains out of scope: only [`llm::StreamingLocalLlmEngine`] itself
//! is provided — a chat model still has to *implement*
//! [`ene_infer::StreamingLocalModel`] to benefit from it (whisper, Kokoro,
//! and GGUF embedding have no reason to: none of them produce incremental
//! partial output today), and grammar-constrained (JSON-schema) chat
//! completion is deliberately never routed through the streaming path —
//! [`crate::LlmProvider::create_chat_stream`] does not accept a
//! `json_schema` argument at all, so a caller that needs one complete JSON
//! document uses [`crate::LlmProvider::chat_completion`] instead, which
//! still returns it as a single value, not fragments to reassemble.

pub mod descriptor;
pub mod llm;
pub mod resource;
pub mod stt;
pub mod tts;

pub use descriptor::{
    Capability, CapabilitySet, ConcurrencyHint, EngineDescriptor, EngineId, ResourceBudgets,
    ResourceClass,
};
pub use llm::{
    DEFAULT_CHUNK_BUFFER, LlmChatRequest, LlmChatResponse, LocalLlmEngine, StreamingLocalLlmEngine,
};
pub use resource::{ResourceRegistry, default_permits};
pub use stt::{LocalSttEngine, SttTranscribeRequest};
pub use tts::{DEFAULT_CHUNK_SAMPLES, LocalTtsEngine, TtsSynthesisRequest, TtsSynthesisResponse};
