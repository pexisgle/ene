//! IPC-backed LLM provider bridging the plugin wire protocol to `ene_ai::LlmProvider`.
//!
//! [`IpcLlmProvider`] holds a shared connection to a plugin binary and
//! translates `LlmProvider` trait calls into `CreateChatStream` /
//! `ChatCompletion` IPC messages.
//!
//! ## Concurrency
//!
//! A single reader task per connection demultiplexes every incoming response
//! by `request_id`/variant into per-request waiters and stream channels.
//! The connection is shared as a plain `Arc<IpcPluginConnection>` with **no
//! external lock**: every request-path method takes `&self`, and the socket
//! write half is guarded by its own short-lived lock released *before* the
//! response is awaited. Stream chunks arrive on a dedicated channel
//! routed by the reader, so tool calls and other requests are never serialized
//! behind a long-running LLM stream. The stream's translation task
//! [`JoinHandle`] is tracked and aborted when the stream is dropped, ensuring
//! prompt cleanup on cancellation.
//!
//! ## Retry
//!
//! Transient failures (transport errors) on `chat_completion` and stream
//! establishment are retried according to the [`RetryPolicy`] supplied by the
//! factory, matching the OpenAI provider's behavior.
//!
//! ## Concurrency admission control
//!
//! The process boundary protects the *host* from a misbehaving plugin — a
//! bad plugin cannot exhaust the host's tokio blocking pool — but nothing
//! protects the *plugin* from the host opening unbounded concurrent requests
//! against it. [`ConcurrencyLimiter`] enforces the
//! [`ConcurrencyHint`](ene_plugin_proto::ConcurrencyHint) the plugin declared
//! during the handshake (or the safe serial default if it declared none):
//! `max_in_flight` requests may run at once, up to `queue_depth` additional
//! callers may wait for a permit, and anything beyond that fails fast with
//! [`LlmProviderError::Busy`] rather than growing the wait queue unboundedly.
//!
//! The limiter is built once per (plugin, provider kind) in
//! [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory) and
//! shared (via `Arc`) across every [`IpcLlmProvider`] instance the factory
//! subsequently creates, since a fresh provider is built per call
//! (`create_task_chat_provider`) — a limiter scoped to a single provider
//! instance would never actually bound anything. The acquired permit is
//! held for the duration of `chat_completion`, and for streams it is moved
//! into [`IpcChatStream`] so `Drop` releases it whether the stream ends
//! naturally or is cancelled mid-flight (see that type's docs).

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use ene_ai::RetryPolicy;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmCompletion, LlmMessage, LlmResponseChunk, LlmToolCallChunk};
use ene_plugin_proto::{ConcurrencyHint, PluginIpcResponse};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_stream::{Stream, wrappers::ReceiverStream};

use crate::error::PluginHostError;
use crate::ipc_plugin::IpcPluginConnection;

/// Bounds concurrent in-flight requests against a single plugin-supplied
/// provider, per the declared [`ConcurrencyHint`].
///
/// `max_in_flight` permits may be held at once; up to `queue_depth`
/// additional callers may wait for a permit to free up. Once both are
/// exhausted, [`acquire`](Self::acquire) fails fast with
/// [`LlmProviderError::Busy`] instead of letting the wait queue grow
/// unboundedly — the same fail-fast-over-queue-forever discipline
/// `ene-infer` applies to local inference engines, applied here to the
/// plugin IPC boundary.
pub struct ConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
    /// Number of callers currently waiting for a permit (i.e. that already
    /// lost the fast-path `try_acquire` race). Bounded by `queue_depth`.
    waiting: AtomicU32,
    queue_depth: u32,
}

impl ConcurrencyLimiter {
    /// Builds a limiter enforcing `hint`.
    ///
    /// `max_in_flight` is clamped to at least 1: a plugin declaring `0`
    /// would otherwise deadlock every request against this provider
    /// forever, which is a worse failure mode than the clamp.
    pub(crate) fn new(hint: ConcurrencyHint) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(hint.max_in_flight.max(1) as usize)),
            waiting: AtomicU32::new(0),
            queue_depth: hint.queue_depth,
        }
    }

    /// Acquires a permit, waiting if necessary (bounded by `queue_depth`).
    ///
    /// Fast path: if a permit is immediately available, returns it without
    /// touching the waiter count at all. Otherwise, reserves one of the
    /// `queue_depth` wait slots (failing fast with
    /// [`LlmProviderError::Busy`] if none remain) and then waits for a
    /// permit to free up.
    pub(crate) async fn acquire(
        &self,
        kind: &str,
    ) -> Result<OwnedSemaphorePermit, LlmProviderError> {
        if let Ok(permit) = Arc::clone(&self.semaphore).try_acquire_owned() {
            return Ok(permit);
        }

        let mut current = self.waiting.load(Ordering::Acquire);
        loop {
            if current >= self.queue_depth {
                tracing::debug!(
                    component = "ConcurrencyLimiter",
                    provider_kind = %kind,
                    queue_depth = self.queue_depth,
                    "provider at its concurrency limit; rejecting request"
                );
                return Err(LlmProviderError::Busy {
                    queue_depth: self.queue_depth as usize,
                });
            }
            match self.waiting.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }

        let result = Arc::clone(&self.semaphore).acquire_owned().await;
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        result.map_err(|_| {
            LlmProviderError::Provider(format!("provider '{kind}' concurrency limiter closed"))
        })
    }
}

/// An `LlmProvider` that delegates to a plugin binary over IPC.
///
/// Created by [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory)
/// during `PluginHostManager` startup.
pub struct IpcLlmProvider {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    model: String,
    max_tokens: Option<u32>,
    provider_config: serde_json::Value,
    retry_policy: RetryPolicy,
    /// Context window the plugin advertised for this provider kind
    /// (`LlmProviderSpec.context_window`), surfaced through
    /// [`LlmProvider::context_window`] so prompt packing can budget against the
    /// model's real limit. `None` when the plugin declared none.
    context_window: Option<u32>,
    /// Shared with every other `IpcLlmProvider` instance created by the same
    /// factory for the same (plugin, kind) pair, so the concurrency bound
    /// holds across the many short-lived provider instances
    /// `create_task_chat_provider` builds (one per call), not just within a
    /// single instance's lifetime.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcLlmProvider {
    /// Creates a new IPC-backed LLM provider.
    ///
    /// `limiter` should be the same `Arc<ConcurrencyLimiter>` shared by every
    /// provider instance the owning factory creates for this (plugin, kind)
    /// pair — see the module docs.
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        model: String,
        max_tokens: Option<u32>,
        provider_config: serde_json::Value,
        retry_policy: RetryPolicy,
        context_window: Option<u32>,
        limiter: Arc<ConcurrencyLimiter>,
    ) -> Self {
        Self {
            kind,
            conn,
            model,
            max_tokens,
            provider_config,
            retry_policy,
            context_window,
            limiter,
        }
    }
}

/// Maps a [`PluginHostError`] into the [`LlmProviderError`] domain.
///
/// Transport failures become [`LlmProviderError::Network`] (retryable);
/// all other host errors become [`LlmProviderError::Provider`] (not
/// retried by the policy).
fn map_host_error(e: PluginHostError) -> LlmProviderError {
    match e {
        PluginHostError::TransportFailed { message } => LlmProviderError::Network(message),
        other => LlmProviderError::Provider(other.to_string()),
    }
}

/// A chat stream that aborts its background reader task when dropped.
///
/// Wraps the per-request [`ReceiverStream`] and the reader's [`JoinHandle`].
/// Dropping the stream (e.g. user cancellation) aborts the reader, releasing
/// the connection for other requests.
///
/// When the stream is dropped *before* it reached a natural terminal message
/// (`completed == false`), the drop handler also sends a best-effort
/// `CancelStream` IPC request so the plugin stops generating the abandoned
/// stream server-side. Natural completion sets `completed` to avoid a
/// pointless cancel round-trip after `StreamEnd`/`StreamError`.
///
/// `_permit` holds this stream's [`ConcurrencyLimiter`] slot for as long as
/// the stream is alive. It is never read — its only purpose is the
/// `OwnedSemaphorePermit` drop glue, which fires as one of this struct's
/// field drops regardless of *why* `IpcChatStream` is being dropped (natural
/// completion or mid-flight cancellation), releasing the slot for the next
/// queued caller in both cases.
struct IpcChatStream {
    rx: ReceiverStream<Result<LlmResponseChunk, LlmProviderError>>,
    reader: Option<JoinHandle<()>>,
    conn: Arc<IpcPluginConnection>,
    request_id: String,
    completed: Arc<std::sync::atomic::AtomicBool>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for IpcChatStream {
    fn drop(&mut self) {
        if let Some(handle) = self.reader.take() {
            handle.abort();
        }
        let conn = Arc::clone(&self.conn);
        let request_id = self.request_id.clone();
        let completed = self.completed.load(std::sync::atomic::Ordering::Acquire);
        // `Drop` is synchronous, so cleanup is dispatched on a detached task.
        // It unregisters the per-request stream channel from the router and,
        // when the stream did not finish naturally, sends a best-effort
        // `CancelStream` so the plugin stops generating the abandoned stream
        // server-side. Natural completion sets `completed` to avoid a
        // pointless cancel round-trip after `StreamEnd`/`StreamError`.
        tokio::spawn(async move {
            conn.close_chat_stream(&request_id);
            if completed {
                return;
            }
            if let Err(e) = conn.cancel_stream(&request_id).await {
                tracing::debug!(
                    component = "IpcLlmProvider",
                    error = %e,
                    request_id = %request_id,
                    "best-effort CancelStream on drop failed"
                );
            }
        });
    }
}

impl Stream for IpcChatStream {
    type Item = Result<LlmResponseChunk, LlmProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Stream::poll_next(Pin::new(&mut self.rx), cx)
    }
}

#[async_trait]
impl ene_ai::LlmProvider for IpcLlmProvider {
    fn name(&self) -> &str {
        &self.kind
    }

    fn context_window(&self) -> Option<u32> {
        self.context_window
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
            .collect();
        let tools_json: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| serde_json::to_value(t).unwrap_or(serde_json::Value::Null))
            .collect();

        let request_id = uuid::Uuid::new_v4().to_string();

        // Admission control: acquire this provider's concurrency
        // slot *before* establishing the stream and *before* the retry loop,
        // so retries never re-acquire while already holding a permit (which
        // would deadlock a max_in_flight=1 provider against itself). The
        // permit moves into `IpcChatStream` below and is held for the
        // stream's entire lifetime.
        let permit = self.limiter.acquire(&self.kind).await?;

        // Establish the stream with retry on transient (transport) failures,
        // matching the OpenAI provider's `establish_sse_connection`.
        // `send_create_chat_stream` takes `&self` and holds the writer lock
        // only for the duration of the write.
        let mut stream_rx = self
            .retry_policy
            .run_with_retry_after(
                LlmProviderError::is_retryable,
                LlmProviderError::retry_after_secs,
                || {
                    let conn = Arc::clone(&self.conn);
                    let request_id = request_id.clone();
                    let kind = self.kind.clone();
                    let provider_config = self.provider_config.clone();
                    let model = self.model.clone();
                    let max_tokens = self.max_tokens;
                    let messages_json = messages_json.clone();
                    let tools_json = tools_json.clone();
                    async move {
                        match conn
                            .send_create_chat_stream(
                                request_id,
                                kind,
                                provider_config,
                                model,
                                max_tokens,
                                messages_json,
                                tools_json,
                            )
                            .await
                        {
                            Ok(rx) => Ok(rx),
                            Err(e @ PluginHostError::TransportFailed { .. }) => {
                                // The stream is likely broken; reconnect so the
                                // next retry attempt starts from a clean
                                // connection (mirrors `do_request_with_timeout`).
                                if let Err(re) = conn.reconnect().await {
                                    tracing::warn!(
                                        component = "IpcLlmProvider",
                                        error = %re,
                                        "reconnect after stream transport failure failed"
                                    );
                                }
                                Err(map_host_error(e))
                            }
                            Err(e) => Err(map_host_error(e)),
                        }
                    }
                },
            )
            .await?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LlmResponseChunk, LlmProviderError>>(32);

        // Spawn a translation task that reads from the router-provided receiver
        // and converts `PluginIpcResponse` stream messages into
        // `LlmResponseChunk`. The single reader task routes all
        // `StreamChunk` / `StreamEnd` / `StreamError` for this `request_id`
        // into `stream_rx`, so no separate connection lock is needed here.
        let rid = request_id.clone();
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed_clone = Arc::clone(&completed);
        let reader = tokio::spawn(async move {
            while let Some(response) = stream_rx.recv().await {
                match response {
                    PluginIpcResponse::StreamChunk {
                        request_id: chunk_rid,
                        text_delta,
                        tool_calls_delta,
                        usage,
                    } if chunk_rid == rid => {
                        let tool_calls = if tool_calls_delta.is_empty() {
                            None
                        } else {
                            Some(
                                tool_calls_delta
                                    .iter()
                                    .enumerate()
                                    .map(|(i, v)| parse_tool_call_delta(v, i))
                                    .collect(),
                            )
                        };
                        let chunk = LlmResponseChunk {
                            text_delta: if text_delta.is_empty() {
                                None
                            } else {
                                Some(text_delta)
                            },
                            tool_calls_delta: tool_calls,
                            usage,
                        };
                        match tx.try_send(Ok(chunk)) {
                            Ok(()) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                tracing::warn!(
                                    component = "IpcLlmProvider",
                                    "Stream chunk dropped due to full channel (backpressure)"
                                );
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                break;
                            }
                        }
                    }
                    PluginIpcResponse::StreamEnd {
                        request_id: end_rid,
                    } if end_rid == rid => {
                        completed_clone.store(true, std::sync::atomic::Ordering::Release);
                        break;
                    }
                    PluginIpcResponse::StreamError {
                        request_id: err_rid,
                        message,
                    } if err_rid == rid => {
                        completed_clone.store(true, std::sync::atomic::Ordering::Release);
                        drop(tx.send(Err(LlmProviderError::Provider(message))).await);
                        break;
                    }
                    PluginIpcResponse::Error { message, .. } => {
                        completed_clone.store(true, std::sync::atomic::Ordering::Release);
                        drop(tx.send(Err(LlmProviderError::Provider(message))).await);
                        break;
                    }
                    _ => {
                        // Unrelated response; skip.
                    }
                }
            }
        });

        Ok(Box::pin(IpcChatStream {
            rx: ReceiverStream::new(rx),
            reader: Some(reader),
            conn: Arc::clone(&self.conn),
            request_id: request_id.clone(),
            completed,
            _permit: permit,
        }))
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
            .collect();

        // Admission control: held for the entire call below,
        // including all retry attempts, so a retry never re-acquires while
        // already holding a permit (which would deadlock a
        // max_in_flight=1 provider against itself). Released when this
        // function returns, on every path (success, error, or panic
        // unwind).
        let _permit = self.limiter.acquire(&self.kind).await?;

        // Retry transient (transport) failures with the same policy as the
        // OpenAI path. The connection's own `do_request` already
        // reconnects once on transport failure; this outer policy adds
        // backoff and additional attempts for persistent transient errors.
        self.retry_policy
            .run_with_retry_after(
                LlmProviderError::is_retryable,
                LlmProviderError::retry_after_secs,
                || {
                    let conn = Arc::clone(&self.conn);
                    let kind = self.kind.clone();
                    let provider_config = self.provider_config.clone();
                    let model = self.model.clone();
                    let max_tokens = self.max_tokens;
                    let messages_json = messages_json.clone();
                    let json_schema = json_schema.clone();
                    async move {
                        let request_id = uuid::Uuid::new_v4().to_string();
                        conn.chat_completion(
                            request_id,
                            kind,
                            provider_config,
                            model,
                            max_tokens,
                            messages_json,
                            json_schema,
                        )
                        .await
                        .map_err(map_host_error)
                    }
                },
            )
            .await
            .map(|(text, usage)| LlmCompletion { text, usage })
    }
}

/// Parses a JSON tool-call delta into an [`LlmToolCallChunk`].
fn parse_tool_call_delta(value: &serde_json::Value, index: usize) -> LlmToolCallChunk {
    LlmToolCallChunk {
        index: value
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map_or(index, |v| v as usize),
        id: value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        arguments: value
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests use unwrap/expect/panic for concise failure messages"
)]
mod concurrency_limiter_tests {
    use std::time::Duration;

    use ene_ai::error::LlmProviderError;
    use ene_plugin_proto::ConcurrencyHint;

    use super::ConcurrencyLimiter;

    #[test]
    fn default_hint_is_serial_with_shallow_queue() {
        let hint = ConcurrencyHint::default();
        assert_eq!(hint.max_in_flight, 1);
        assert_eq!(hint.queue_depth, 2);
    }

    #[tokio::test]
    async fn acquire_succeeds_immediately_under_the_limit() {
        let limiter = ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 2,
            queue_depth: 0,
        });
        let _p1 = limiter.acquire("test").await.unwrap();
        let _p2 = limiter.acquire("test").await.unwrap();
        // Both in-flight slots are held; a third caller with no queue depth
        // must fail fast rather than block.
        let err = limiter.acquire("test").await.unwrap_err();
        assert!(matches!(err, LlmProviderError::Busy { .. }));
    }

    #[tokio::test]
    async fn over_limit_with_no_queue_depth_fails_fast() {
        let limiter = ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 0,
        });
        let _permit = limiter.acquire("test").await.unwrap();
        let err = limiter
            .acquire("test")
            .await
            .expect_err("second caller must be rejected: no in-flight slot, no queue depth");
        match err {
            LlmProviderError::Busy { queue_depth } => assert_eq!(queue_depth, 0),
            other => panic!("expected Busy, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn queued_caller_is_admitted_once_a_permit_frees_up() {
        let limiter = std::sync::Arc::new(ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 1,
        }));
        let permit = limiter.acquire("test").await.unwrap();

        // A second caller has queue room (queue_depth: 1) and should wait
        // rather than being rejected immediately.
        let waiter_limiter = std::sync::Arc::clone(&limiter);
        let waiter = tokio::spawn(async move { waiter_limiter.acquire("test").await });

        // Give the waiter task a chance to start waiting before releasing.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "waiter should still be blocked while the only permit is held"
        );

        drop(permit);
        let result = waiter.await.unwrap();
        assert!(
            result.is_ok(),
            "queued caller must be admitted once the permit is released"
        );
    }

    #[tokio::test]
    async fn beyond_queue_depth_fails_fast_instead_of_queueing_unboundedly() {
        let limiter = std::sync::Arc::new(ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 1,
        }));
        let _permit = limiter.acquire("test").await.unwrap();

        let queued_limiter = std::sync::Arc::clone(&limiter);
        let queued = tokio::spawn(async move { queued_limiter.acquire("test").await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        // A second caller, beyond max_in_flight + queue_depth, must be
        // rejected immediately rather than growing the wait queue further.
        let err = limiter.acquire("test").await.unwrap_err();
        assert!(matches!(err, LlmProviderError::Busy { .. }));

        queued.abort();
    }

    /// Simulates the `IpcChatStream` drop path: a permit held by a
    /// long-lived value (standing in for the stream) is released as soon as
    /// that value is dropped — whether from natural completion or, as here,
    /// early/mid-flight cancellation — freeing the slot for the next caller.
    #[tokio::test]
    async fn permit_is_released_when_holder_is_dropped_mid_flight() {
        let limiter = ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 0,
        });
        let permit = limiter.acquire("test").await.unwrap();

        assert!(limiter.acquire("test").await.is_err());

        // Drop simulates `IpcChatStream`'s `_permit` field being dropped when
        // the stream itself is dropped mid-flight (cancellation), not only
        // on natural `StreamEnd`/`StreamError` completion.
        drop(permit);

        assert!(limiter.acquire("test").await.is_ok());
    }
}
