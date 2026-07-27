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
//! - [`llm::LocalLlmEngine`] — blanket [`crate::LlmProvider`].
//! - [`tts::LocalTtsEngine`] — blanket [`crate::TtsProvider`].
//! - [`stt::LocalSttEngine`] — blanket [`crate::SttProvider`].
//! - [`descriptor::EngineDescriptor`] — declared capability, concurrency,
//!   and resource-class metadata, replacing "call it and see if it errors"
//!   (e.g. the tools-non-empty runtime check `create_chat_stream` used to
//!   need) with an upfront query.
//! - [`resource::ResourceRegistry`] — one shared admission semaphore per
//!   distinct [`descriptor::ResourceClass`], so independently-constructed
//!   engines that contend on the same physical resource (most importantly:
//!   the same GPU device) are admission-controlled together instead of each
//!   having a lock that only protects itself.
//!
//! # Where this does *not* fully work
//!
//! [`ene_infer::LocalModel::run`] is a one-shot synchronous call: it hands
//! back exactly one `Response` when it returns, with no channel for partial
//! progress. That is fine for [`crate::TtsProvider::synthesize_stream`] (see
//! [`tts`]'s docs) and for embeddings, but it means
//! [`crate::LlmProvider::create_chat_stream`] can never deliver *real*
//! token-by-token streaming through this adapter — only a single completed
//! reply wrapped in a one-item `Stream`, same as
//! `LocalLlamaCppProvider::create_chat_stream` does today. A local chat
//! engine that wants genuine incremental delivery has no way to get it from
//! [`llm::LocalLlmEngine`]; the only way out today is to *not* use this
//! adapter and hand-roll an async impl again — reopening exactly the
//! concurrency hazards this crate exists to close. Closing that gap
//! properly is a `ene-infer` change (e.g. a callback/channel the worker can
//! push partial results through while `run` is still executing), out of
//! scope for this stage.

pub mod descriptor;
pub mod llm;
pub mod resource;
pub mod stt;
pub mod tts;

pub use descriptor::{
    Capability, CapabilitySet, ConcurrencyHint, EngineDescriptor, EngineId, ResourceBudgets,
    ResourceClass,
};
pub use llm::{LlmChatRequest, LlmChatResponse, LocalLlmEngine};
pub use resource::{ResourceRegistry, default_permits};
pub use stt::{LocalSttEngine, SttTranscribeRequest};
pub use tts::{DEFAULT_CHUNK_SAMPLES, LocalTtsEngine, TtsSynthesisRequest, TtsSynthesisResponse};
