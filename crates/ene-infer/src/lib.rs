//! # ene-infer
//!
//! Framework for running synchronous, single-threaded local inference
//! models (LLM, embedding, STT, TTS, ...) behind a uniform async API,
//! without letting each provider hand-roll its own concurrency.
//!
//! ## Why concurrency is centralized here
//!
//! Hand-rolled provider concurrency keeps going wrong in the same ways:
//! wrapping `spawn_blocking` in `tokio::time::timeout` (the blocking work is
//! not cancellable, so a "timed out" job keeps running while holding a lock on
//! the model), using `block_in_place` (panics off a multi-thread runtime
//! worker), or taking cached state out of a mutex around a `spawn_blocking`
//! call (a cancelled or panicking task loses that state permanently).
//!
//! This crate's answer is to stop letting providers express concurrency at
//! all. Implementors write [`LocalModel`], a plain synchronous trait taking
//! `&mut self`; the framework supplies the async wrapper, the dedicated
//! worker thread, the bounded queue, cooperative cancellation, and panic
//! recovery.
//!
//! ## Worker invariants
//!
//! - The model is moved into a dedicated `std::thread` and owned there —
//!   never `Arc<Mutex<_>>`, never tokio's blocking pool.
//! - The queue ([`EngineHandle::submit`]) is bounded and never blocks: a
//!   full queue fails fast with [`EngineError::Busy`].
//! - There is exactly one timeout ([`EngineConfig::job_timeout`]), enforced
//!   *cooperatively* from inside the worker, starting when the job begins
//!   executing (queue-wait time does not count against it). There is
//!   deliberately no second, outer `tokio::time::timeout` wrapping
//!   [`EngineHandle::submit`] anywhere in this crate — that outer/inner
//!   double timeout, where the outer timeout fires while the inner blocking
//!   work keeps running, is precisely the bug this crate exists to remove.
//! - [`LocalModel::run`] is wrapped in `catch_unwind`. A panic yields
//!   [`EngineError::EngineDown`] for that job, and the worker rebuilds the
//!   model from the factory given to [`EngineHandle::spawn`] before
//!   accepting the next one.
//! - [`LocalModel::reset`] runs after every job that returns from `run`
//!   without panicking — success, model error, timeout, cancellation, and
//!   caller-gone all count.
//!
//! ## Known limitation: `catch_unwind` cannot contain an `abort()`
//!
//! Panic recovery only works for genuine Rust panics that unwind. Native
//! code that calls `abort()` directly (for example llama.cpp's
//! `GGML_ASSERT`) takes the whole process down regardless of `catch_unwind`
//! — there is no way to intercept that from safe Rust. This is an accepted
//! limit of the design, not an oversight: providers whose native
//! dependencies abort on invariant violations are expected to validate
//! inputs before they reach that code.
#![warn(missing_docs)]
#![cfg_attr(
    all(test, feature = "test-util"),
    expect(
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests (gated on test-util, see src/tests.rs) use expect()/panic! for concise assertions"
    )
)]

mod config;
mod context;
mod error;
mod handle;
mod model;
mod stream;

#[cfg(feature = "test-util")]
/// A generic conformance battery for [`LocalModel`] implementations,
/// exercising the queueing, cancellation, and panic-recovery invariants
/// this crate guarantees. See the module docs for the (small) additional
/// contract a model's test-only request/response types must satisfy.
pub mod conformance;

#[cfg(all(test, feature = "test-util"))]
mod tests;

pub use config::EngineConfig;
pub use context::{JobContext, StopReason};
pub use error::EngineError;
pub use handle::EngineHandle;
pub use model::{LocalModel, StreamingLocalModel};
pub use stream::{ChunkReceiver, ChunkSink};
