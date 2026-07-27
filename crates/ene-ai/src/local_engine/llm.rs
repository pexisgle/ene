//! Blanket [`LlmProvider`] adapter over an [`ene_infer::EngineHandle`].
//!
//! This is the async `impl` written exactly once. A future local chat model
//! implements [`ene_infer::LocalModel`] plus the two small conversions below
//! ([`From<LlmChatRequest>`] for its request type, [`Into<LlmChatResponse>`]
//! for its response type) and gets [`LlmProvider`] for free — no
//! `spawn_blocking`, no hand-rolled timeout, no `Arc<Mutex<_>>` around the
//! model.

use std::pin::Pin;

use async_trait::async_trait;
use ene_infer::{EngineError, EngineHandle};
use ene_plugin_proto::ToolSpec;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use super::descriptor::{Capability, EngineDescriptor};
use super::resource::ResourceRegistry;
use crate::error::LlmProviderError;
use crate::message::{LlmMessage, LlmResponseChunk, UserMessagePart};
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
}

/// Maps an [`ene_infer::EngineError`] to [`LlmProviderError`], preserving
/// which case fired rather than collapsing to a string.
///
/// `EngineError::Model` is the one case that cannot be typed further here:
/// `M::Error` is opaque to this adapter (any [`ene_infer::LocalModel`]'s own
/// error type), so it becomes `LlmProviderError::LocalLlm` via `Display` —
/// the same bucket `ene-ai-local`'s existing local providers already use for
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
        // mechanism today for a synchronous `run` call to hand partial
        // tokens back to the async side before it returns. This wraps the
        // single completed reply in a one-item stream, exactly like
        // `LocalLlamaCppProvider::create_chat_stream` does today — it is not
        // token-by-token streaming, and cannot become so without a change to
        // `ene-infer`'s `LocalModel` contract (see this module's docs on
        // `Capability::Streaming`).
        let text = self.chat_completion(messages, None).await?;
        Ok(Box::pin(tokio_stream::once(Ok(LlmResponseChunk {
            text_delta: Some(text),
            tool_calls_delta: None,
        }))))
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
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
        Ok(response.into().text)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ene_infer::{EngineConfig, JobContext, LocalModel};
    use ene_plugin_proto::{ToolName, ToolSpec};

    use super::{LlmChatRequest, LlmChatResponse, LocalLlmEngine, map_engine_error};
    use crate::local_engine::descriptor::{
        Capability, CapabilitySet, EngineDescriptor, ResourceClass,
    };
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
            Self { text: r.0 }
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
        assert_eq!(reply, "hello");
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
        crate::local_engine::resource::ResourceRegistry::configure_all(
            &crate::local_engine::descriptor::ResourceBudgets::new().with_permits(resource, 8),
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
}
