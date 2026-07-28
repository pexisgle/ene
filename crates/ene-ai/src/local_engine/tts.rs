//! Blanket [`TtsProvider`] adapter over an [`ene_infer::EngineHandle`].
//!
//! [`ene_infer::LocalModel::run`] is one-shot: a synthesis job produces one
//! full PCM buffer, not incremental audio. [`TtsProvider::synthesize_stream`]
//! still returns a `Stream`, so this adapter preserves the shape
//! `crates/ene-voice/src/local_tts.rs` already uses — a single batch
//! inference whose PCM buffer is sliced into fixed-size chunks and pushed
//! through an `mpsc` channel — rather than inventing true streaming
//! synthesis. Unlike `local_tts.rs`, no `spawn_blocking` is needed anywhere
//! in this adapter: `EngineHandle::submit` is already async, and the actual
//! blocking inference happens on `ene-infer`'s own dedicated worker thread.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use ene_infer::{EngineError, EngineHandle};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use super::descriptor::EngineDescriptor;
use super::resource::ResourceRegistry;
use crate::traits::{AudioProviderError, TtsChunk, TtsProvider};

/// Owned text-to-speech synthesis request.
#[derive(Debug, Clone)]
pub struct TtsSynthesisRequest {
    /// Text to synthesize.
    pub text: String,
}

/// A completed synthesis job's full PCM buffer, sliced into [`TtsChunk`]s at
/// the adapter boundary (see this module's docs).
#[derive(Debug, Clone)]
pub struct TtsSynthesisResponse {
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

fn map_engine_error<E: std::error::Error>(err: EngineError<E>) -> AudioProviderError {
    match err {
        EngineError::Busy { queue_depth } => AudioProviderError::Busy { queue_depth },
        EngineError::Timeout { .. } => AudioProviderError::Timeout,
        EngineError::Cancelled => AudioProviderError::Cancelled,
        EngineError::EngineDown { reason } => {
            AudioProviderError::Provider(format!("engine down: {reason}"))
        }
        EngineError::Model(model_err) => AudioProviderError::Provider(model_err.to_string()),
    }
}

/// Number of PCM samples per streamed [`TtsChunk`] when none is given to
/// [`LocalTtsEngine::new`]. Matches `local_tts.rs`'s `CHUNK_SAMPLES` (~0.25s
/// at 24kHz), a reasonable default independent of any particular model's
/// sample rate.
pub const DEFAULT_CHUNK_SAMPLES: usize = 6_000;

/// Blanket [`TtsProvider`] for any [`ene_infer::LocalModel`] whose request
/// and response types can be built from / converted to
/// [`TtsSynthesisRequest`] / [`TtsSynthesisResponse`].
///
/// Holds `Arc<EngineHandle<M>>` (rather than the handle directly) because
/// [`Self::synthesize_stream`] hands the handle to a detached `tokio::spawn`
/// task so it can return the `Stream` immediately, before inference
/// completes — [`EngineHandle`] itself does not implement `Clone`
/// (deliberately; see its docs), so callers that need to share it wrap it in
/// an `Arc`, which is exactly what this adapter does internally.
pub struct LocalTtsEngine<M: ene_infer::LocalModel> {
    handle: Arc<EngineHandle<M>>,
    descriptor: EngineDescriptor,
    chunk_samples: usize,
}

impl<M: ene_infer::LocalModel> LocalTtsEngine<M> {
    /// Wraps an already-spawned [`EngineHandle`] with its descriptor, using
    /// [`DEFAULT_CHUNK_SAMPLES`] for chunking.
    #[must_use]
    pub fn new(handle: EngineHandle<M>, descriptor: EngineDescriptor) -> Self {
        Self::with_chunk_samples(handle, descriptor, DEFAULT_CHUNK_SAMPLES)
    }

    /// As [`Self::new`], with an explicit PCM samples-per-chunk size.
    #[must_use]
    pub fn with_chunk_samples(
        handle: EngineHandle<M>,
        descriptor: EngineDescriptor,
        chunk_samples: usize,
    ) -> Self {
        Self {
            handle: Arc::new(handle),
            descriptor,
            chunk_samples: chunk_samples.max(1),
        }
    }
}

#[async_trait]
impl<M> TtsProvider for LocalTtsEngine<M>
where
    M: ene_infer::LocalModel,
    M::Request: From<TtsSynthesisRequest>,
    M::Response: Into<TtsSynthesisResponse>,
{
    fn name(&self) -> &str {
        self.descriptor.id.as_str()
    }

    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
        AudioProviderError,
    > {
        let (tx, rx) = mpsc::channel::<Result<TtsChunk, AudioProviderError>>(4);
        let handle = Arc::clone(&self.handle);
        let resource = self.descriptor.resource;
        let chunk_samples = self.chunk_samples;
        let request: M::Request = TtsSynthesisRequest {
            text: text.to_string(),
        }
        .into();

        tokio::spawn(async move {
            let permit = match ResourceRegistry::acquire(resource).await {
                Ok(permit) => permit,
                Err(e) => {
                    let msg = format!("resource admission semaphore closed: {e}");
                    drop(tx.send(Err(AudioProviderError::Provider(msg))).await);
                    return;
                }
            };
            let outcome = handle.submit(request, CancellationToken::new()).await;
            drop(permit);

            let response = match outcome {
                Ok(response) => response.into(),
                Err(e) => {
                    drop(tx.send(Err(map_engine_error(e))).await);
                    return;
                }
            };
            let TtsSynthesisResponse { pcm, sample_rate } = response;
            for slice in pcm.chunks(chunk_samples) {
                let chunk = TtsChunk {
                    pcm: slice.to_vec(),
                    sample_rate,
                };
                // Stop if the consumer dropped the receiver.
                if tx.send(Ok(chunk)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ene_infer::{EngineConfig, JobContext, LocalModel};
    use tokio_stream::StreamExt;

    use super::{LocalTtsEngine, TtsSynthesisRequest, TtsSynthesisResponse, map_engine_error};
    use crate::local_engine::descriptor::{EngineDescriptor, ResourceClass};
    use crate::traits::{AudioProviderError, TtsProvider};

    #[derive(Debug, thiserror::Error)]
    #[error("mock tts model error: {0}")]
    struct MockTtsError(String);

    #[derive(Debug, Clone)]
    enum MockTtsRequest {
        /// Produces `samples` samples of silence.
        Silence(usize),
        Slow(Duration),
        Fail,
        /// Scripted for `ene_infer::conformance::run_all` (see
        /// `ConformanceRequest` below) — distinct from `Slow`/`Fail` because
        /// the battery needs to script an optional panic too.
        Scripted {
            run_for: Duration,
            then_panic: bool,
        },
    }

    impl From<TtsSynthesisRequest> for MockTtsRequest {
        fn from(req: TtsSynthesisRequest) -> Self {
            match req.text.as_str() {
                "__slow__" => Self::Slow(Duration::from_millis(300)),
                "__fail__" => Self::Fail,
                other => Self::Silence(other.len().max(1) * 1000),
            }
        }
    }

    impl ene_infer::conformance::ConformanceRequest for MockTtsRequest {
        fn scripted(run_for: Duration, then_panic: bool) -> Self {
            Self::Scripted {
                run_for,
                then_panic,
            }
        }
    }

    impl Default for MockTtsRequest {
        /// Required by `ConformanceRequest: Default`; never actually run
        /// with this value (the battery always builds requests via
        /// `scripted`), so any variant works.
        fn default() -> Self {
            Self::Silence(0)
        }
    }

    #[derive(Debug)]
    struct MockTtsResponse {
        samples: usize,
        resets_seen: usize,
    }

    impl From<MockTtsResponse> for TtsSynthesisResponse {
        fn from(r: MockTtsResponse) -> Self {
            Self {
                pcm: vec![0.0_f32; r.samples],
                sample_rate: 24_000,
            }
        }
    }

    impl ene_infer::conformance::ConformanceResponse for MockTtsResponse {
        fn resets_seen(&self) -> usize {
            self.resets_seen
        }
    }

    #[derive(Debug, Default)]
    struct MockTtsModel {
        resets_seen: usize,
    }

    impl LocalModel for MockTtsModel {
        type Request = MockTtsRequest;
        type Response = MockTtsResponse;
        type Error = MockTtsError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "mock-tts"
        }

        fn run(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            match req {
                MockTtsRequest::Silence(samples) => Ok(MockTtsResponse {
                    samples,
                    resets_seen: self.resets_seen,
                }),
                MockTtsRequest::Fail => Err(MockTtsError("scripted failure".to_string())),
                MockTtsRequest::Slow(run_for) => {
                    let start = Instant::now();
                    loop {
                        if ctx.should_stop().is_some() {
                            return Err(MockTtsError("stopped".to_string()));
                        }
                        ctx.tick();
                        if start.elapsed() >= run_for {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(MockTtsResponse {
                        samples: 100,
                        resets_seen: self.resets_seen,
                    })
                }
                MockTtsRequest::Scripted {
                    run_for,
                    then_panic,
                } => {
                    assert!(!then_panic, "scripted panic for conformance testing");
                    let start = Instant::now();
                    loop {
                        if ctx.should_stop().is_some() {
                            return Err(MockTtsError("stopped".to_string()));
                        }
                        ctx.tick();
                        if start.elapsed() >= run_for {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(MockTtsResponse {
                        samples: 0,
                        resets_seen: self.resets_seen,
                    })
                }
            }
        }

        fn reset(&mut self) {
            self.resets_seen += 1;
        }
    }

    fn provider(
        resource: ResourceClass,
        cfg: EngineConfig,
        chunk_samples: usize,
    ) -> LocalTtsEngine<MockTtsModel> {
        let handle = ene_infer::EngineHandle::spawn(|| Ok(MockTtsModel::default()), cfg);
        let descriptor = EngineDescriptor::new(
            "mock-tts",
            crate::local_engine::descriptor::CapabilitySet::empty()
                .with(crate::local_engine::descriptor::Capability::Tts),
            resource,
        );
        LocalTtsEngine::with_chunk_samples(handle, descriptor, chunk_samples)
    }

    #[tokio::test]
    async fn synthesize_stream_slices_pcm_into_fixed_chunks() {
        let p = provider(
            ResourceClass::Gpu { device: 301 },
            EngineConfig::default(),
            4,
        );
        // "abc" -> 3000 samples of silence, chunked at 4 samples/chunk.
        let mut stream = p
            .synthesize_stream("abc")
            .await
            .expect("synthesize_stream returns immediately");
        let mut total = 0usize;
        let mut chunks = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.expect("chunk ok");
            assert_eq!(chunk.sample_rate, 24_000);
            assert!(chunk.pcm.len() <= 4);
            total += chunk.pcm.len();
            chunks += 1;
        }
        assert_eq!(total, 3000);
        assert_eq!(chunks, 3000 / 4);
    }

    #[tokio::test]
    async fn synthesize_stream_model_error_maps_to_provider() {
        let p = provider(
            ResourceClass::Gpu { device: 302 },
            EngineConfig::default(),
            super::DEFAULT_CHUNK_SAMPLES,
        );
        let mut stream = p
            .synthesize_stream("__fail__")
            .await
            .expect("synthesize_stream returns immediately even for a job that will fail");
        let first = stream.next().await.expect("one item: the error");
        assert!(matches!(first, Err(AudioProviderError::Provider(_))));
    }

    #[tokio::test]
    async fn synthesize_stream_deadline_maps_to_timeout() {
        let p = provider(
            ResourceClass::Gpu { device: 303 },
            EngineConfig::new(4, Duration::from_millis(20)),
            super::DEFAULT_CHUNK_SAMPLES,
        );
        let mut stream = p
            .synthesize_stream("__slow__")
            .await
            .expect("synthesize_stream returns immediately");
        let first = stream.next().await.expect("one item: the timeout");
        assert!(matches!(first, Err(AudioProviderError::Timeout)));
    }

    #[tokio::test]
    async fn independent_engines_sharing_a_resource_class_serialize() {
        let resource = ResourceClass::Gpu { device: 304 }; // unconfigured: default 1 permit
        let a = provider(
            resource,
            EngineConfig::default(),
            super::DEFAULT_CHUNK_SAMPLES,
        );
        let b = provider(
            resource,
            EngineConfig::default(),
            super::DEFAULT_CHUNK_SAMPLES,
        );

        let started = Instant::now();
        let stream_a = a.synthesize_stream("__slow__").await.expect("stream a");
        let stream_b = b.synthesize_stream("__slow__").await.expect("stream b");
        let (drain_a, drain_b) = tokio::join!(
            tokio_stream::StreamExt::collect::<Vec<_>>(stream_a),
            tokio_stream::StreamExt::collect::<Vec<_>>(stream_b),
        );
        assert!(drain_a.iter().all(Result::is_ok));
        assert!(drain_b.iter().all(Result::is_ok));

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

        let busy: EngineError<MockTtsError> = EngineError::Busy { queue_depth: 2 };
        assert!(matches!(
            map_engine_error(busy),
            AudioProviderError::Busy { queue_depth: 2 }
        ));

        let cancelled: EngineError<MockTtsError> = EngineError::Cancelled;
        assert!(matches!(
            map_engine_error(cancelled),
            AudioProviderError::Cancelled
        ));

        let model: EngineError<MockTtsError> = EngineError::Model(MockTtsError("boom".to_string()));
        assert!(matches!(
            map_engine_error(model),
            AudioProviderError::Provider(_)
        ));
    }

    /// Runs the generic queueing/cancellation/panic-recovery/reset battery
    /// against `MockTtsModel` itself (the `LocalModel`, not the
    /// `LocalTtsEngine` adapter around it) — this is what keeps this
    /// blanket adapter's assumptions about `ene_infer::LocalModel` correct
    /// as new `LocalModel` implementations (voice engines, future
    /// providers) land beside it.
    #[tokio::test]
    async fn mock_tts_model_passes_conformance_battery() {
        ene_infer::conformance::run_all(MockTtsModel::default).await;
    }
}
