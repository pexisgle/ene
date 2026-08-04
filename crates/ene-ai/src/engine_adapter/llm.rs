//! Blanket [`LlmProvider`] adapter over an [`ene_infer::EngineHandle`].
//!
//! This is the async `impl` written exactly once. A future local chat model
//! implements [`ene_infer::LocalModel`] plus the two small conversions below
//! ([`From<LlmChatRequest>`] for its request type, [`Into<LlmChatResponse>`]
//! for its response type) and gets [`LlmProvider`] for free — no
//! `spawn_blocking`, no hand-rolled timeout, no `Arc<Mutex<_>>` around the
//! model.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use ene_infer::{ChunkReceiver, EngineError, EngineHandle};
use ene_plugin_proto::ToolSpec;
use tokio::sync::OwnedSemaphorePermit;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use super::descriptor::{Capability, EngineDescriptor};
use super::resource::ResourceRegistry;
use crate::error::LlmProviderError;
use crate::message::{LlmCompletion, LlmMessage, LlmResponseChunk, UserMessagePart};
use crate::traits::LlmProvider;

/// Owned chat-completion request, built from whatever [`LlmProvider`]
/// receives as borrowed slices.
///
/// [`ene_infer::LocalModel::Request`] must be `Send + 'static` (it crosses
/// the worker-thread channel), so this cannot borrow — every field is
/// cloned out of the caller's arguments once, at the adapter boundary.
#[derive(Debug, Clone)]
pub struct LlmChatRequest {
    /// The conversation so far.
    pub messages: Vec<LlmMessage>,
    /// Tool/function definitions available to the model, if any.
    pub tools: Vec<ToolSpec>,
    /// A JSON schema the response must conform to, if the caller asked for
    /// constrained output.
    pub json_schema: Option<serde_json::Value>,
}

/// A completed chat turn's assistant text.
#[derive(Debug, Clone)]
pub struct LlmChatResponse {
    /// The full assistant reply text.
    pub text: String,
    /// Token usage the local engine counted itself, if any. Local
    /// models (llama.cpp) know their exact prompt/completion token counts;
    /// engines that cannot count leave this `None` and callers fall back to a
    /// character-based estimate.
    pub usage: Option<ene_plugin_proto::TokenUsage>,
}

/// Maps an [`ene_infer::EngineError`] to [`LlmProviderError`], preserving
/// which case fired rather than collapsing to a string.
///
/// `EngineError::Model` is the one case that cannot be typed further here:
/// `M::Error` is opaque to this adapter (any [`ene_infer::LocalModel`]'s own
/// error type), so it becomes `LlmProviderError::LocalLlm` via `Display` —
/// the same bucket the local-llm plugin's local providers already use for
/// "the model itself failed".
fn map_engine_error<E: std::error::Error>(err: EngineError<E>) -> LlmProviderError {
    match err {
        EngineError::Busy { queue_depth } => LlmProviderError::Busy { queue_depth },
        EngineError::Timeout { .. } => LlmProviderError::Timeout,
        EngineError::Cancelled => LlmProviderError::Cancelled,
        EngineError::EngineDown { reason } => {
            LlmProviderError::Provider(format!("engine down: {reason}"))
        }
        EngineError::Model(model_err) => LlmProviderError::LocalLlm(model_err.to_string()),
    }
}

/// Whether any message in `messages` carries image content.
fn has_vision_input(messages: &[LlmMessage]) -> bool {
    messages.iter().any(|m| match m {
        LlmMessage::User { parts } => parts
            .iter()
            .any(|p| matches!(p, UserMessagePart::Image { .. })),
        LlmMessage::System { .. } | LlmMessage::Assistant { .. } | LlmMessage::Tool { .. } => false,
    })
}

/// Blanket [`LlmProvider`] for any [`ene_infer::LocalModel`] whose request
/// and response types can be built from / converted to the small
/// [`LlmChatRequest`] / [`LlmChatResponse`] contract above.
pub struct LocalLlmEngine<M: ene_infer::LocalModel> {
    handle: EngineHandle<M>,
    descriptor: EngineDescriptor,
}

impl<M: ene_infer::LocalModel> LocalLlmEngine<M> {
    /// Wraps an already-spawned [`EngineHandle`] with its descriptor.
    #[must_use]
    pub fn new(handle: EngineHandle<M>, descriptor: EngineDescriptor) -> Self {
        Self { handle, descriptor }
    }

    /// The underlying handle, for callers whose model needs to submit
    /// request shapes beyond [`LlmChatRequest`] on the same worker/model
    /// instance this adapter wraps.
    #[must_use]
    pub fn handle(&self) -> &EngineHandle<M> {
        &self.handle
    }

    /// This engine's declared capability/concurrency/resource metadata.
    #[must_use]
    pub fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }
}

#[async_trait]
impl<M> LlmProvider for LocalLlmEngine<M>
where
    M: ene_infer::LocalModel,
    M::Request: From<LlmChatRequest>,
    M::Response: Into<LlmChatResponse>,
{
    fn name(&self) -> &str {
        self.descriptor.id.as_str()
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        if !tools.is_empty() && !self.descriptor.capabilities.contains(Capability::Tools) {
            return Err(LlmProviderError::Provider(format!(
                "engine '{}' does not support tool calls",
                self.descriptor.id
            )));
        }
        if has_vision_input(messages) && !self.descriptor.capabilities.contains(Capability::Vision)
        {
            return Err(LlmProviderError::Provider(format!(
                "engine '{}' does not support image input",
                self.descriptor.id
            )));
        }

        // `LocalModel::run` is one-shot, not incremental: there is no
        // mechanism for a synchronous `run` call to hand partial tokens back
        // to the async side before it returns. This wraps the single
        // completed reply in a one-item stream — not token-by-token
        // streaming. A model that wants real incremental delivery should
        // implement `ene_infer::StreamingLocalModel` and be wrapped in
        // `StreamingLocalLlmEngine` instead (see that type's docs, and
        // `crate::engine_adapter`'s module docs on why this had to be a
        // separate type rather than a second `impl` here).
        let completion = self.chat_completion(messages, None).await?;
        Ok(Box::pin(tokio_stream::once(Ok(LlmResponseChunk {
            text_delta: Some(completion.text),
            tool_calls_delta: None,
            usage: completion.usage,
        }))))
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        if has_vision_input(messages) && !self.descriptor.capabilities.contains(Capability::Vision)
        {
            return Err(LlmProviderError::Provider(format!(
                "engine '{}' does not support image input",
                self.descriptor.id
            )));
        }

        let permit = ResourceRegistry::acquire(self.descriptor.resource)
            .await
            .map_err(|e| {
                LlmProviderError::Provider(format!("resource admission semaphore closed: {e}"))
            })?;

        let request = LlmChatRequest {
            messages: messages.to_vec(),
            tools: Vec::new(),
            json_schema,
        };
        let outcome = self
            .handle
            .submit(request.into(), CancellationToken::new())
            .await;
        drop(permit);

        let response = outcome.map_err(map_engine_error)?;
        let response: LlmChatResponse = response.into();
        Ok(LlmCompletion {
            text: response.text,
            usage: response.usage,
        })
    }
}

/// Number of chunks buffered between the worker thread and the async
/// consumer when none is given to [`StreamingLocalLlmEngine::new`]. This is
/// the backpressure knob described on [`ene_infer::EngineHandle::submit_stream`]:
/// once full, the worker blocks producing further tokens until the consumer
/// drains one, the job's deadline elapses, or the consumer drops the stream —
/// it never grows past this bound. 16 is small enough that a stalled
/// consumer is noticed quickly, generous enough that ordinary token
/// production never blocks on a consumer that is merely a little behind.
pub const DEFAULT_CHUNK_BUFFER: usize = 16;

/// Sibling of [`LocalLlmEngine`] for models that can deliver *real*
/// token-by-token output via [`ene_infer::StreamingLocalModel`], instead of
/// [`LocalLlmEngine::create_chat_stream`]'s one-item-stream fallback.
///
/// This has to be a separate type, not a second `impl LlmProvider for
/// LocalLlmEngine<M>` block bounded on `M: StreamingLocalModel`: Rust has no
/// stable specialization, so two blanket impls of the same trait for the
/// same type — even gated on a subtrait bound — would conflict, and there
/// would be no way to pick "the real-streaming one" over "the one-item
/// fallback" for a model that happens to implement both. Wrapping a
/// [`LocalLlmEngine<M>`] (rather than duplicating its fields) means
/// [`Self::chat_completion`] and the capability/vision checks stay identical
/// to the non-streaming adapter by construction — only [`Self::create_chat_stream`]
/// differs.
///
/// A model that does *not* implement [`ene_infer::StreamingLocalModel`]
/// simply cannot be wrapped in this type — `M: StreamingLocalModel` is
/// required at the type level, not checked at runtime — so
/// [`LocalLlmEngine`] keeps working, completely unchanged, for every other
/// local model in this workspace (whisper, Kokoro, GGUF embedding, and any
/// future non-streaming chat model).
pub struct StreamingLocalLlmEngine<M: ene_infer::StreamingLocalModel> {
    inner: LocalLlmEngine<M>,
    chunk_buffer: usize,
}

impl<M: ene_infer::StreamingLocalModel> StreamingLocalLlmEngine<M> {
    /// Wraps an already-spawned [`EngineHandle`] with its descriptor, using
    /// [`DEFAULT_CHUNK_BUFFER`] for the chunk channel's bound.
    #[must_use]
    pub fn new(handle: EngineHandle<M>, descriptor: EngineDescriptor) -> Self {
        Self::with_chunk_buffer(handle, descriptor, DEFAULT_CHUNK_BUFFER)
    }

    /// As [`Self::new`], with an explicit chunk-buffer bound.
    #[must_use]
    pub fn with_chunk_buffer(
        handle: EngineHandle<M>,
        descriptor: EngineDescriptor,
        chunk_buffer: usize,
    ) -> Self {
        Self {
            inner: LocalLlmEngine::new(handle, descriptor),
            chunk_buffer: chunk_buffer.max(1),
        }
    }

    /// The underlying handle — see [`LocalLlmEngine::handle`].
    #[must_use]
    pub fn handle(&self) -> &EngineHandle<M> {
        self.inner.handle()
    }

    /// This engine's declared capability/concurrency/resource metadata.
    #[must_use]
    pub fn descriptor(&self) -> &EngineDescriptor {
        self.inner.descriptor()
    }
}

/// Bridges [`ChunkReceiver`] to `tokio_stream`'s [`Stream`], mapping each
/// chunk through `Into<LlmResponseChunk>` and each terminal
/// [`ene_infer::EngineError`] through [`map_engine_error`].
///
/// This is the "thin local newtype" [`ene_infer::stream`]'s module docs
/// describe: `ChunkReceiver` deliberately does not implement `Stream` itself
/// (`ene-infer` stays a leaf crate, no `futures`/`tokio-stream` dependency),
/// so the one-line bridge lives here instead, in a crate that already
/// depends on `tokio-stream`.
///
/// Also holds the resource-admission permit for as long as the stream itself
/// is alive, not just while the job was being submitted: a streaming job's
/// admission window is "while chunks are still being produced", the same
/// semaphore-lifetime shape [`LocalLlmEngine::chat_completion`] already uses
/// for its one, non-streaming call.
struct ChunkReceiverStream<M: ene_infer::StreamingLocalModel> {
    receiver: ChunkReceiver<M::Chunk, M::Error>,
    _permit: OwnedSemaphorePermit,
}

impl<M: ene_infer::StreamingLocalModel> ChunkReceiverStream<M> {
    fn new(receiver: ChunkReceiver<M::Chunk, M::Error>, permit: OwnedSemaphorePermit) -> Self {
        Self {
            receiver,
            _permit: permit,
        }
    }
}

impl<M> Stream for ChunkReceiverStream<M>
where
    M: ene_infer::StreamingLocalModel,
    M::Chunk: Into<LlmResponseChunk>,
{
    type Item = Result<LlmResponseChunk, LlmProviderError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `Self` is `Unpin` (every field is: `mpsc::Receiver` and
        // `DropGuard` inside `ChunkReceiver`, plus `OwnedSemaphorePermit`,
        // none of which is self-referential), so projecting straight to a
        // plain `&mut Self` is sound.
        match self.get_mut().receiver.poll_recv(cx) {
            Poll::Ready(Some(Ok(chunk))) => Poll::Ready(Some(Ok(chunk.into()))),
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(map_engine_error(err)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[async_trait]
impl<M> LlmProvider for StreamingLocalLlmEngine<M>
where
    M: ene_infer::StreamingLocalModel,
    M::Request: From<LlmChatRequest>,
    M::Response: Into<LlmChatResponse>,
    M::Chunk: Into<LlmResponseChunk>,
{
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        let descriptor = self.inner.descriptor();
        if !tools.is_empty() && !descriptor.capabilities.contains(Capability::Tools) {
            return Err(LlmProviderError::Provider(format!(
                "engine '{}' does not support tool calls",
                descriptor.id
            )));
        }
        if has_vision_input(messages) && !descriptor.capabilities.contains(Capability::Vision) {
            return Err(LlmProviderError::Provider(format!(
                "engine '{}' does not support image input",
                descriptor.id
            )));
        }

        let permit = ResourceRegistry::acquire(descriptor.resource)
            .await
            .map_err(|e| {
                LlmProviderError::Provider(format!("resource admission semaphore closed: {e}"))
            })?;

        // Tools are validated above but not forwarded into the request,
        // matching `LocalLlmEngine::chat_completion`'s existing behavior —
        // no local model in this workspace consumes `LlmChatRequest::tools`
        // yet (see that method's own construction of this same request
        // shape). Fixing that is unrelated to streaming and out of scope
        // here.
        let request = LlmChatRequest {
            messages: messages.to_vec(),
            tools: Vec::new(),
            json_schema: None,
        };
        let receiver = self.inner.handle().submit_stream(
            request.into(),
            CancellationToken::new(),
            self.chunk_buffer,
        );
        Ok(Box::pin(ChunkReceiverStream::<M>::new(receiver, permit)))
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        self.inner.chat_completion(messages, json_schema).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ene_infer::{EngineConfig, JobContext, LocalModel};
    use ene_plugin_proto::{ToolName, ToolSpec};

    use super::{
        LlmChatRequest, LlmChatResponse, LocalLlmEngine, StreamingLocalLlmEngine, map_engine_error,
    };
    use crate::engine_adapter::{Capability, CapabilitySet, EngineDescriptor, ResourceClass};
    use crate::message::{LlmMessage, UserMessagePart};
    use crate::traits::LlmProvider;

    #[derive(Debug, thiserror::Error)]
    #[error("mock chat model error: {0}")]
    struct MockChatError(String);

    #[derive(Debug, Clone)]
    enum MockChatRequest {
        Echo(String),
        Slow(Duration),
        Fail,
    }

    fn last_user_text(messages: &[LlmMessage]) -> String {
        messages
            .iter()
            .rev()
            .find_map(|m| match m {
                LlmMessage::User { parts } => parts.iter().find_map(|p| match p {
                    UserMessagePart::Text { text } => Some(text.clone()),
                    UserMessagePart::Image { .. } => None,
                }),
                LlmMessage::System { .. }
                | LlmMessage::Assistant { .. }
                | LlmMessage::Tool { .. } => None,
            })
            .unwrap_or_default()
    }

    impl From<LlmChatRequest> for MockChatRequest {
        fn from(req: LlmChatRequest) -> Self {
            match last_user_text(&req.messages).as_str() {
                "__slow__" => Self::Slow(Duration::from_millis(300)),
                "__fail__" => Self::Fail,
                other => Self::Echo(other.to_string()),
            }
        }
    }

    #[derive(Debug)]
    struct MockChatResponse(String);

    impl From<MockChatResponse> for LlmChatResponse {
        fn from(r: MockChatResponse) -> Self {
            Self {
                text: r.0,
                usage: None,
            }
        }
    }

    #[derive(Debug, Default)]
    struct MockChatModel;

    impl LocalModel for MockChatModel {
        type Request = MockChatRequest;
        type Response = MockChatResponse;
        type Error = MockChatError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "mock-chat"
        }

        fn run(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            match req {
                MockChatRequest::Echo(text) => Ok(MockChatResponse(text)),
                MockChatRequest::Fail => Err(MockChatError("scripted failure".to_string())),
                MockChatRequest::Slow(run_for) => {
                    let start = Instant::now();
                    loop {
                        if ctx.should_stop().is_some() {
                            return Err(MockChatError("stopped".to_string()));
                        }
                        ctx.tick();
                        if start.elapsed() >= run_for {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(MockChatResponse("slow done".to_string()))
                }
            }
        }
    }

    fn text_message(text: &str) -> LlmMessage {
        LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: text.to_string(),
            }],
        }
    }

    fn image_message() -> LlmMessage {
        LlmMessage::User {
            parts: vec![UserMessagePart::Image {
                base64_image_data: "data:image/png;base64,AAAA".to_string(),
            }],
        }
    }

    fn engine(
        resource: ResourceClass,
        capabilities: CapabilitySet,
        cfg: EngineConfig,
    ) -> LocalLlmEngine<MockChatModel> {
        let handle = ene_infer::EngineHandle::spawn(|| Ok(MockChatModel), cfg);
        let descriptor = EngineDescriptor::new("mock-chat", capabilities, resource);
        LocalLlmEngine::new(handle, descriptor)
    }

    #[tokio::test]
    async fn chat_completion_echoes_text() {
        let provider = engine(
            ResourceClass::Gpu { device: 201 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let reply = provider
            .chat_completion(&[text_message("hello")], None)
            .await
            .expect("chat completion succeeds");
        assert_eq!(reply.text, "hello");
        assert_eq!(reply.usage, None);
    }

    #[tokio::test]
    async fn create_chat_stream_rejects_tools_when_not_declared() {
        let provider = engine(
            ResourceClass::Gpu { device: 202 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let tools = [ToolSpec::new(
            ToolName::new("search"),
            String::new(),
            serde_json::json!({}),
        )];
        let result = provider
            .create_chat_stream(&[text_message("hi")], &tools)
            .await;
        let Err(err) = result else {
            panic!("tools not declared as a capability must be rejected")
        };
        assert!(matches!(err, crate::error::LlmProviderError::Provider(_)));
    }

    #[tokio::test]
    async fn create_chat_stream_allows_tools_when_declared() {
        let provider = engine(
            ResourceClass::Gpu { device: 203 },
            CapabilitySet::empty()
                .with(Capability::Chat)
                .with(Capability::Tools),
            EngineConfig::default(),
        );
        let tools = [ToolSpec::new(
            ToolName::new("search"),
            String::new(),
            serde_json::json!({}),
        )];
        let mut stream = provider
            .create_chat_stream(&[text_message("hi")], &tools)
            .await
            .expect("tools declared as a capability must be allowed");
        let first = tokio_stream::StreamExt::next(&mut stream)
            .await
            .expect("one chunk")
            .expect("chunk ok");
        assert_eq!(first.text_delta.as_deref(), Some("hi"));
    }

    #[tokio::test]
    async fn chat_completion_rejects_vision_when_not_declared() {
        let provider = engine(
            ResourceClass::Gpu { device: 204 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let err = provider
            .chat_completion(&[image_message()], None)
            .await
            .expect_err("image input without Vision capability must be rejected");
        assert!(matches!(err, crate::error::LlmProviderError::Provider(_)));
    }

    #[tokio::test]
    async fn chat_completion_model_error_maps_to_local_llm() {
        let provider = engine(
            ResourceClass::Gpu { device: 205 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let err = provider
            .chat_completion(&[text_message("__fail__")], None)
            .await
            .expect_err("scripted model failure surfaces");
        assert!(matches!(err, crate::error::LlmProviderError::LocalLlm(_)));
    }

    #[tokio::test]
    async fn chat_completion_deadline_maps_to_timeout() {
        let provider = engine(
            ResourceClass::Gpu { device: 206 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::new(4, Duration::from_millis(20)),
        );
        let err = provider
            .chat_completion(&[text_message("__slow__")], None)
            .await
            .expect_err("job exceeding job_timeout must surface Timeout");
        assert!(matches!(err, crate::error::LlmProviderError::Timeout));
    }

    #[tokio::test]
    async fn chat_completion_busy_when_queue_full() {
        // Generous resource budget so the shared semaphore is never this
        // test's bottleneck — only the engine's own bounded queue
        // (`queue_depth: 1`) should produce `Busy`.
        let resource = ResourceClass::Gpu { device: 207 };
        crate::engine_adapter::resource::ResourceRegistry::configure_all(
            &crate::engine_adapter::descriptor::ResourceBudgets::new().with_permits(resource, 8),
        );
        let provider = std::sync::Arc::new(engine(
            resource,
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::new(1, Duration::from_secs(10)),
        ));

        let first = {
            let provider = std::sync::Arc::clone(&provider);
            tokio::spawn(async move {
                provider
                    .chat_completion(&[text_message("__slow__")], None)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(75)).await;
        let second = {
            let provider = std::sync::Arc::clone(&provider);
            tokio::spawn(async move {
                provider
                    .chat_completion(&[text_message("__slow__")], None)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let third = provider
            .chat_completion(&[text_message("third")], None)
            .await;
        assert!(
            matches!(
                third,
                Err(crate::error::LlmProviderError::Busy { queue_depth: 1 })
            ),
            "expected Busy once the queue is at capacity, got {third:?}"
        );

        let first = first.await.expect("task panicked");
        let second = second.await.expect("task panicked");
        assert!(
            first.is_ok(),
            "expected first slow job to succeed: {first:?}"
        );
        assert!(
            second.is_ok(),
            "expected queued second job to succeed: {second:?}"
        );
    }

    #[tokio::test]
    async fn independent_engines_sharing_a_resource_class_serialize() {
        // Two entirely separate `EngineHandle`s (separate worker threads,
        // separate bounded queues) that declare the *same* `ResourceClass`
        // must still serialize against each other via the shared semaphore
        // — this is the scenario the whole registry exists for (e.g. a
        // decision LLM and a vision mmproj model both offloading to GPU 0).
        let resource = ResourceClass::Gpu { device: 208 }; // unconfigured: default_permits == 1
        let engine_a = engine(
            resource,
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let engine_b = engine(
            resource,
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );

        let started = Instant::now();
        let a = tokio::spawn(async move {
            engine_a
                .chat_completion(&[text_message("__slow__")], None)
                .await
        });
        let b = tokio::spawn(async move {
            engine_b
                .chat_completion(&[text_message("__slow__")], None)
                .await
        });
        let (a, b) = tokio::join!(a, b);
        let a = a.expect("task panicked");
        let b = b.expect("task panicked");
        assert!(a.is_ok(), "engine A's job should still succeed: {a:?}");
        assert!(b.is_ok(), "engine B's job should still succeed: {b:?}");

        // Each scripted job runs ~300ms; if the shared semaphore truly
        // serialized them, total wall time is close to 2x that, not 1x.
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(450),
            "two engines sharing one ResourceClass appear to have run \
             concurrently instead of serialized: elapsed {elapsed:?}"
        );
    }

    #[test]
    fn map_engine_error_preserves_variant() {
        use ene_infer::EngineError;

        let busy: EngineError<MockChatError> = EngineError::Busy { queue_depth: 3 };
        assert!(matches!(
            map_engine_error(busy),
            crate::error::LlmProviderError::Busy { queue_depth: 3 }
        ));

        let timeout: EngineError<MockChatError> = EngineError::Timeout {
            after: Duration::from_secs(1),
        };
        assert!(matches!(
            map_engine_error(timeout),
            crate::error::LlmProviderError::Timeout
        ));

        let cancelled: EngineError<MockChatError> = EngineError::Cancelled;
        assert!(matches!(
            map_engine_error(cancelled),
            crate::error::LlmProviderError::Cancelled
        ));

        let down: EngineError<MockChatError> = EngineError::EngineDown {
            reason: "worker died".to_string(),
        };
        assert!(matches!(
            map_engine_error(down),
            crate::error::LlmProviderError::Provider(_)
        ));

        let model: EngineError<MockChatError> =
            EngineError::Model(MockChatError("boom".to_string()));
        assert!(matches!(
            map_engine_error(model),
            crate::error::LlmProviderError::LocalLlm(_)
        ));
    }

    use ene_infer::{ChunkSink, StreamingLocalModel};

    #[derive(Debug, thiserror::Error)]
    #[error("mock streaming chat model error: {0}")]
    struct MockStreamChatError(String);

    #[derive(Debug, Clone)]
    struct MockStreamChatRequest {
        chunks: Vec<String>,
        pause: Duration,
        fail_after: Option<usize>,
    }

    impl From<LlmChatRequest> for MockStreamChatRequest {
        fn from(req: LlmChatRequest) -> Self {
            match last_user_text(&req.messages).as_str() {
                "__many__" => Self {
                    chunks: ["a", "b", "c", "d"]
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                    pause: Duration::from_millis(30),
                    fail_after: None,
                },
                "__fail__" => Self {
                    chunks: ["x", "y", "z"].iter().map(|s| (*s).to_string()).collect(),
                    pause: Duration::ZERO,
                    fail_after: Some(2),
                },
                other => Self {
                    chunks: vec![other.to_string()],
                    pause: Duration::ZERO,
                    fail_after: None,
                },
            }
        }
    }

    #[derive(Debug)]
    struct MockStreamChatResponse(String);

    impl From<MockStreamChatResponse> for LlmChatResponse {
        fn from(r: MockStreamChatResponse) -> Self {
            Self {
                text: r.0,
                usage: None,
            }
        }
    }

    #[derive(Debug, Default)]
    struct MockStreamChatModel;

    impl LocalModel for MockStreamChatModel {
        type Request = MockStreamChatRequest;
        type Response = MockStreamChatResponse;
        type Error = MockStreamChatError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "mock-stream-chat"
        }

        fn run(
            &mut self,
            req: Self::Request,
            _ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            Ok(MockStreamChatResponse(req.chunks.concat()))
        }
    }

    impl StreamingLocalModel for MockStreamChatModel {
        type Chunk = String;

        fn run_streaming(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
            sink: &ChunkSink<Self::Chunk, Self::Error>,
        ) -> Result<(), Self::Error> {
            for (i, chunk) in req.chunks.iter().enumerate() {
                if ctx.should_stop().is_some() {
                    return Err(MockStreamChatError("stopped".to_string()));
                }
                ctx.tick();
                if !req.pause.is_zero() {
                    std::thread::sleep(req.pause);
                }
                if sink.send(chunk.clone(), ctx).is_err() {
                    return Err(MockStreamChatError("stopped".to_string()));
                }
                if req.fail_after == Some(i + 1) {
                    return Err(MockStreamChatError(
                        "scripted mid-stream failure".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    fn streaming_engine(
        resource: ResourceClass,
        capabilities: CapabilitySet,
        cfg: EngineConfig,
    ) -> StreamingLocalLlmEngine<MockStreamChatModel> {
        let handle = ene_infer::EngineHandle::spawn(|| Ok(MockStreamChatModel), cfg);
        let descriptor = EngineDescriptor::new("mock-stream-chat", capabilities, resource);
        StreamingLocalLlmEngine::new(handle, descriptor)
    }

    #[tokio::test]
    async fn streaming_create_chat_stream_delivers_incremental_chunks() {
        let provider = streaming_engine(
            ResourceClass::Gpu { device: 301 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let mut stream = provider
            .create_chat_stream(&[text_message("__many__")], &[])
            .await
            .expect("create_chat_stream");

        let started = Instant::now();
        let first = tokio_stream::StreamExt::next(&mut stream)
            .await
            .expect("stream ended before its first chunk")
            .expect("first chunk ok");
        let first_latency = started.elapsed();
        assert_eq!(first.text_delta.as_deref(), Some("a"));
        assert!(
            first_latency < Duration::from_millis(90),
            "first chunk took {first_latency:?}; expected close to one pause, not all four"
        );

        let mut texts = vec![first.text_delta.expect("text delta")];
        while let Some(item) = tokio_stream::StreamExt::next(&mut stream).await {
            texts.push(item.expect("chunk ok").text_delta.expect("text delta"));
        }
        assert_eq!(texts, vec!["a", "b", "c", "d"]);
        assert!(
            started.elapsed() >= Duration::from_millis(90),
            "draining all four chunks finished too fast for them to have been paced one at a \
             time instead of produced upfront"
        );
    }

    #[tokio::test]
    async fn streaming_create_chat_stream_surfaces_mid_stream_model_error() {
        let provider = streaming_engine(
            ResourceClass::Gpu { device: 302 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let mut stream = provider
            .create_chat_stream(&[text_message("__fail__")], &[])
            .await
            .expect("create_chat_stream");

        let mut ok_chunks = Vec::new();
        let mut saw_error = false;
        while let Some(item) = tokio_stream::StreamExt::next(&mut stream).await {
            match item {
                Ok(chunk) => {
                    assert!(!saw_error, "received a chunk after the terminal error");
                    ok_chunks.push(chunk.text_delta.expect("text delta"));
                }
                Err(err) => {
                    assert!(
                        matches!(err, crate::error::LlmProviderError::LocalLlm(_)),
                        "expected a mapped model error, got {err:?}"
                    );
                    saw_error = true;
                }
            }
        }
        assert_eq!(ok_chunks, vec!["x", "y"]);
        assert!(saw_error, "expected a terminal error after 2 chunks");
    }

    #[tokio::test]
    async fn streaming_create_chat_stream_rejects_tools_when_not_declared() {
        let provider = streaming_engine(
            ResourceClass::Gpu { device: 303 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let tools = [ToolSpec::new(
            ToolName::new("search"),
            String::new(),
            serde_json::json!({}),
        )];
        let result = provider
            .create_chat_stream(&[text_message("hi")], &tools)
            .await;
        let Err(err) = result else {
            panic!("tools not declared as a capability must be rejected")
        };
        assert!(matches!(err, crate::error::LlmProviderError::Provider(_)));
    }

    #[tokio::test]
    async fn streaming_chat_completion_still_works_through_the_streaming_adapter() {
        let provider = streaming_engine(
            ResourceClass::Gpu { device: 304 },
            CapabilitySet::empty().with(Capability::Chat),
            EngineConfig::default(),
        );
        let reply = provider
            .chat_completion(&[text_message("solo")], None)
            .await
            .expect("chat completion succeeds");
        assert_eq!(reply.text, "solo");
    }
}
