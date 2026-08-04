//! Screen-image vision summarization, decoupled from the turn-execution
//! actor mailbox.
//!
//! Before this split, `EneCommand::SummarizeScreenImage` carried a raw RGB8
//! buffer (`width * height * 3` bytes, up to several megabytes for a
//! 1920x1080 capture) through the actor's command enum, and the actual
//! (potentially multi-second) vision-model inference ran inside the actor's
//! `vision_tasks` `JoinSet` — a `JoinSet` whose own doc comment admitted it
//! existed only because the work "must not block the command loop".
//!
//! [`VisionHandle`] removes both problems: the raw buffer never enters
//! [`crate::handle::EneCommand`], and the expensive vision inference happens
//! directly here, awaited by the caller, never inside the actor's command
//! loop or any actor-owned `JoinSet`.
//!
//! What *does* still cross the mailbox (deliberately, see the PR
//! description for the tradeoff) is a small, payload-free
//! `PrepareVisionSummary` request/reply pair: the vision path shares the
//! same "runtime busy" gate (`active_turn` / in-flight proactive decision)
//! and the same lazily-initialized local GGUF vision model as the proactive
//! scheduler, both of which are actor-owned state. `PrepareVisionSummary`
//! performs that busy-check and lazy init, then hands back a cheap
//! `Arc`-cloned model handle plus the rendered system/user prompts. A
//! second fire-and-forget message, `StashProactiveScreenImage`, hands the
//! actor the *encoded* JPEG data URI (not the raw buffer) so the next
//! proactive generation turn can still attach it, preserving prior
//! behavior.

use crate::handle::EneCommand;
use crate::public_api::PublicApiError;
use ene_ai::message::{LlmMessage, UserMessagePart};
use ene_ai::traits::LlmProvider;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

/// Maximum accepted pixel count for a screen capture (1920x1080).
///
/// Public so the desktop compositor can assert its composite budget stays at
/// or below this limit (see `ene-desktop`'s `MAX_COMPOSITE_PIXELS` test).
pub const MAX_PIXELS: u64 = 1920 * 1080;

/// Result of a successful [`EneCommand::PrepareVisionSummary`] round trip:
/// everything [`VisionHandle::summarize_screen_image`] needs to run the
/// actual vision inference outside the actor.
pub struct VisionPrepared {
    /// Cloned handle to the local vision-capable model.
    pub local: Arc<dyn LlmProvider>,
    /// Rendered system prompt for the screen-summary task.
    pub system: String,
    /// Rendered user prompt (includes the privacy-safe app label).
    pub user: String,
    /// Cancellation token for this specific vision request. The actor mints
    /// a fresh token per [`EneCommand::PrepareVisionSummary`] reply and
    /// cancels it when a new user turn starts, so a long-running vision
    /// inference does not keep the local plugin busy behind a request the
    /// user has already moved past. Observed here via `select!`; when it
    /// fires, the stream is dropped and the host sends `CancelStream` to the
    /// plugin.
    pub cancel: CancellationToken,
}

/// Handle for screen-image vision summarization.
///
/// Obtained via [`crate::EneHandle::vision`]. Cheap to clone (wraps a
/// single `Arc`d command sender).
#[derive(Clone)]
pub struct VisionHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
}

/// Non-image context for the screen-summary vision call.
///
/// All fields are small text / flags; raw pixels never cross the mailbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenSummaryHints {
    /// The frame composites a 100%-scale cursor ROI next to the 50% overview.
    pub roi_composited: bool,
    /// The active window looks like a code editor / terminal (title heuristics).
    pub code_window: bool,
    /// Raw OCR text extracted from the focus region; `None` when no OCR
    /// backend produced a hint.
    pub ocr_text: Option<String>,
}

impl VisionHandle {
    pub(crate) fn new(cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>) -> Self {
        Self { cmd_tx }
    }

    /// Summarize a screen RGB8 capture via the local vision (mmproj) model.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    ///
    /// The `rgb` buffer is encoded to a JPEG data URI and sent to the local
    /// plugin as an image-part chat message; the actual model call never
    /// touches the actor's command mailbox — see the module docs for what
    /// small, payload-free messages still do.
    pub async fn summarize_screen_image(
        &self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        app_label: String,
        hints: ScreenSummaryHints,
    ) -> Result<String, PublicApiError> {
        validate_rgb(width, height, &rgb)?;

        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::PrepareVisionSummary {
                app_label,
                hints,
                reply,
            })
            .map_err(|_| PublicApiError::ActorDead)?;
        let prepared = rx.await.map_err(|_| PublicApiError::ActorDead)??;

        // Encode once: the same data URI feeds the stash and the inference
        // message (a 1080p frame takes ~100 ms to encode).
        let data_uri = crate::proactive::rgb_to_jpeg_data_uri(width, height, &rgb)
            .map_err(|e| PublicApiError::Internal { message: e })?;
        // Best-effort stash of the *encoded* frame for the next proactive
        // generation turn (never the raw buffer, and never blocks this
        // call or the actor on failure).
        drop(self.cmd_tx.send(EneCommand::StashProactiveScreenImage {
            data_uri: Some(data_uri.clone()),
        }));

        // The actual (potentially multi-second) inference call happens
        // here, entirely outside the actor's command loop. `prepared.cancel`
        // lets a new user turn abort it early (see `VisionPrepared::cancel`).
        let messages = build_vision_messages(prepared.system, prepared.user, data_uri);
        drain_vision_summary(prepared.local.as_ref(), &messages, &prepared.cancel)
            .await
            .map_err(|e| PublicApiError::Internal {
                message: if matches!(e, ene_ai::error::LlmProviderError::Cancelled) {
                    "vision summarization cancelled".to_string()
                } else {
                    e.to_string()
                },
            })
    }
}

/// Builds the chat messages for a screen summary: the rendered system
/// prompt, then a user turn carrying the rendered text prompt and the
/// encoded frame.
fn build_vision_messages(system: String, user: String, data_uri: String) -> Vec<LlmMessage> {
    vec![
        LlmMessage::System { content: system },
        LlmMessage::User {
            parts: vec![
                UserMessagePart::Text { text: user },
                UserMessagePart::Image {
                    base64_image_data: data_uri,
                },
            ],
        },
    ]
}

/// Streams a screen-summary completion from `provider`, collecting text
/// deltas until the stream ends.
///
/// `cancel` aborts the inference: the stream is dropped (which makes the IPC
/// layer send a best-effort `CancelStream`) and the call fails with
/// [`ene_ai::error::LlmProviderError::Cancelled`].
async fn drain_vision_summary(
    provider: &dyn LlmProvider,
    messages: &[LlmMessage],
    cancel: &CancellationToken,
) -> Result<String, ene_ai::error::LlmProviderError> {
    let mut stream = provider.create_chat_stream(messages, &[]).await?;
    let mut summary = String::new();
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                // Dropping the stream aborts the reader task and sends a
                // best-effort `CancelStream` so the plugin stops generating
                // the abandoned request.
                drop(stream);
                return Err(ene_ai::error::LlmProviderError::Cancelled);
            }
            chunk = stream.next() => match chunk {
                Some(Ok(chunk)) => {
                    if let Some(delta) = chunk.text_delta {
                        summary.push_str(&delta);
                    }
                }
                Some(Err(e)) => return Err(e),
                None => break,
            },
        }
    }
    Ok(summary)
}

/// Validates a screen-capture RGB8 buffer's dimensions and length before it
/// is sent anywhere. Pure and actor-independent.
fn validate_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<(), PublicApiError> {
    let width_u = u64::from(width);
    let height_u = u64::from(height);
    if width == 0 || height == 0 {
        return Err(PublicApiError::Invalid {
            message: "invalid screen image dimensions".to_string(),
        });
    }
    if width_u.saturating_mul(height_u) > MAX_PIXELS {
        return Err(PublicApiError::Invalid {
            message: format!("screen image too large ({width}x{height}; max {MAX_PIXELS} pixels)"),
        });
    }
    let expected = width_u.saturating_mul(height_u).saturating_mul(3);
    let Ok(expected_len) = usize::try_from(expected) else {
        return Err(PublicApiError::Invalid {
            message: "screen image byte length overflows usize".to_string(),
        });
    };
    if rgb.len() != expected_len {
        return Err(PublicApiError::Invalid {
            message: format!(
                "rgb buffer length mismatch (got {}, expected {expected_len})",
                rgb.len()
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ene_ai::error::LlmProviderError;
    use ene_ai::message::{LlmCompletion, LlmResponseChunk};
    use ene_ai::traits::LlmProvider;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;
    use tokio_stream::Stream;

    /// A stub provider whose streams emit a scripted chunk sequence and
    /// record the messages they were created with.
    struct StubVisionProvider {
        chunks: std::sync::Mutex<VecDeque<Result<LlmResponseChunk, LlmProviderError>>>,
        seen_messages: std::sync::Mutex<Vec<LlmMessage>>,
        streams_dropped: Arc<AtomicUsize>,
        /// When true, streams never terminate (cancel test).
        never_ends: bool,
    }

    impl StubVisionProvider {
        fn new(chunks: Vec<Result<LlmResponseChunk, LlmProviderError>>) -> Self {
            Self {
                chunks: std::sync::Mutex::new(VecDeque::from(chunks)),
                seen_messages: std::sync::Mutex::new(Vec::new()),
                streams_dropped: Arc::new(AtomicUsize::new(0)),
                never_ends: false,
            }
        }

        fn never_ending() -> Self {
            Self {
                chunks: std::sync::Mutex::new(VecDeque::new()),
                seen_messages: std::sync::Mutex::new(Vec::new()),
                streams_dropped: Arc::new(AtomicUsize::new(0)),
                never_ends: true,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StubVisionProvider {
        fn name(&self) -> &'static str {
            "vision-stub"
        }

        async fn create_chat_stream(
            &self,
            messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
            LlmProviderError,
        > {
            self.seen_messages
                .lock()
                .expect("test mutex not poisoned")
                .extend_from_slice(messages);
            if self.never_ends {
                return Ok(Box::pin(HangingStream {
                    dropped: Arc::clone(&self.streams_dropped),
                }));
            }
            let chunks = std::mem::take(&mut *self.chunks.lock().expect("test mutex not poisoned"));
            Ok(Box::pin(ChunkStream {
                chunks,
                dropped: Arc::clone(&self.streams_dropped),
            }))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<LlmCompletion, LlmProviderError> {
            Ok(LlmCompletion::text_only("stub".to_string()))
        }
    }

    /// Emits the scripted chunks, then ends. Counts drops so the cancel test
    /// can assert the stream was actually dropped.
    struct ChunkStream {
        chunks: VecDeque<Result<LlmResponseChunk, LlmProviderError>>,
        dropped: Arc<AtomicUsize>,
    }

    impl Stream for ChunkStream {
        type Item = Result<LlmResponseChunk, LlmProviderError>;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.chunks.pop_front())
        }
    }

    impl Drop for ChunkStream {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Never terminates, so only cancellation ends the drain.
    struct HangingStream {
        dropped: Arc<AtomicUsize>,
    }

    impl Stream for HangingStream {
        type Item = Result<LlmResponseChunk, LlmProviderError>;

        fn poll_next(
            self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Pending
        }
    }

    impl Drop for HangingStream {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn delta(text: &str) -> LlmResponseChunk {
        LlmResponseChunk {
            text_delta: Some(text.to_string()),
            tool_calls_delta: None,
            usage: None,
        }
    }

    #[test]
    fn build_vision_messages_has_system_and_image_user() {
        let messages = build_vision_messages(
            "sys".to_string(),
            "user".to_string(),
            "data:image/jpeg".into(),
        );
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0],
            LlmMessage::System {
                content: "sys".to_string()
            }
        );
        assert_eq!(
            messages[1],
            LlmMessage::User {
                parts: vec![
                    UserMessagePart::Text {
                        text: "user".to_string()
                    },
                    UserMessagePart::Image {
                        base64_image_data: "data:image/jpeg".to_string()
                    },
                ],
            }
        );
    }

    #[tokio::test]
    async fn drain_vision_summary_accumulates_text_deltas() {
        let provider = StubVisionProvider::new(vec![Ok(delta("Hello")), Ok(delta(" world"))]);
        let messages = build_vision_messages(String::new(), String::new(), String::new());
        let summary = drain_vision_summary(&provider, &messages, &CancellationToken::new())
            .await
            .expect("summary drains");
        assert_eq!(summary, "Hello world");
        // The stub received exactly the two vision messages.
        let seen = provider
            .seen_messages
            .lock()
            .expect("test mutex not poisoned");
        assert_eq!(seen.len(), 2);
        assert!(matches!(seen[1], LlmMessage::User { .. }));
    }

    #[tokio::test]
    async fn drain_vision_summary_maps_stream_error() {
        let provider = StubVisionProvider::new(vec![
            Ok(delta("partial")),
            Err(LlmProviderError::Provider("boom".to_string())),
        ]);
        let messages = build_vision_messages(String::new(), String::new(), String::new());
        let err = drain_vision_summary(&provider, &messages, &CancellationToken::new())
            .await
            .expect_err("stream error surfaces");
        assert!(matches!(err, LlmProviderError::Provider(message) if message == "boom"));
    }

    #[tokio::test]
    async fn drain_vision_summary_cancel_drops_the_stream() {
        let provider = StubVisionProvider::never_ending();
        let messages = build_vision_messages(String::new(), String::new(), String::new());
        let cancel = CancellationToken::new();
        let cancel_handle = cancel.clone();
        let streams_dropped = Arc::clone(&provider.streams_dropped);
        let task = tokio::spawn(async move {
            drain_vision_summary(&provider, &messages, &cancel_handle).await
        });
        // Let the drain establish the stream, then cancel it.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel.cancel();
        let err = task
            .await
            .expect("drain task completes")
            .expect_err("cancel must fail the drain");
        assert!(matches!(err, LlmProviderError::Cancelled));
        assert_eq!(
            streams_dropped.load(Ordering::SeqCst),
            1,
            "cancelling must drop the stream so CancelStream is sent"
        );
    }

    #[test]
    fn validate_rgb_accepts_exact_buffer() {
        assert!(validate_rgb(2, 2, &[0_u8; 12]).is_ok());
    }

    #[test]
    fn validate_rgb_rejects_length_mismatch_and_oversize() {
        assert!(matches!(
            validate_rgb(2, 2, &[0_u8; 11]),
            Err(PublicApiError::Invalid { .. })
        ));
        assert!(matches!(
            validate_rgb(MAX_PIXELS as u32 + 1, 1, &[0_u8; 3]),
            Err(PublicApiError::Invalid { .. })
        ));
    }
}
