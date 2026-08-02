//! Blanket [`SttProvider`] adapter over an [`ene_infer::EngineHandle`].
//!
//! Transcription is naturally one-shot (a full utterance in, one
//! [`SttResult`] out), so unlike [`super::tts`] there is no chunked-delivery
//! shape to preserve here — this adapter is a direct `submit` and map.

use async_trait::async_trait;
use ene_infer::{EngineError, EngineHandle};
use tokio_util::sync::CancellationToken;

use super::descriptor::EngineDescriptor;
use super::resource::ResourceRegistry;
use crate::traits::{AudioProviderError, SttProvider, SttResult};

/// Owned speech-to-text transcription request.
#[derive(Debug, Clone)]
pub struct SttTranscribeRequest {
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate of `pcm`, in Hz.
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

/// Blanket [`SttProvider`] for any [`ene_infer::LocalModel`] whose request
/// type can be built from [`SttTranscribeRequest`] and whose response
/// converts to [`SttResult`].
pub struct LocalSttEngine<M: ene_infer::LocalModel> {
    handle: EngineHandle<M>,
    descriptor: EngineDescriptor,
}

impl<M: ene_infer::LocalModel> LocalSttEngine<M> {
    /// Wraps an already-spawned [`EngineHandle`] with its descriptor.
    #[must_use]
    pub fn new(handle: EngineHandle<M>, descriptor: EngineDescriptor) -> Self {
        Self { handle, descriptor }
    }
}

#[async_trait]
impl<M> SttProvider for LocalSttEngine<M>
where
    M: ene_infer::LocalModel,
    M::Request: From<SttTranscribeRequest>,
    M::Response: Into<SttResult>,
{
    fn name(&self) -> &str {
        self.descriptor.id.as_str()
    }

    async fn transcribe(
        &self,
        pcm: &[f32],
        sample_rate: u32,
    ) -> Result<SttResult, AudioProviderError> {
        let permit = ResourceRegistry::acquire(self.descriptor.resource)
            .await
            .map_err(|e| {
                AudioProviderError::Provider(format!("resource admission semaphore closed: {e}"))
            })?;

        let request = SttTranscribeRequest {
            pcm: pcm.to_vec(),
            sample_rate,
        };
        let outcome = self
            .handle
            .submit(request.into(), CancellationToken::new())
            .await;
        drop(permit);

        let response = outcome.map_err(map_engine_error)?;
        Ok(response.into())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ene_infer::{EngineConfig, JobContext, LocalModel};

    use super::{LocalSttEngine, SttTranscribeRequest, map_engine_error};
    use crate::engine_adapter::{Capability, CapabilitySet, EngineDescriptor, ResourceClass};
    use crate::traits::{AudioProviderError, SttProvider, SttResult};

    #[derive(Debug, thiserror::Error)]
    #[error("mock stt model error: {0}")]
    struct MockSttError(String);

    #[derive(Debug, Clone)]
    enum MockSttRequest {
        Echo(usize),
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

    impl From<SttTranscribeRequest> for MockSttRequest {
        fn from(req: SttTranscribeRequest) -> Self {
            if req.pcm.len() == 999 {
                Self::Slow(Duration::from_millis(300))
            } else if req.pcm.is_empty() {
                Self::Fail
            } else {
                Self::Echo(req.pcm.len())
            }
        }
    }

    impl ene_infer::conformance::ConformanceRequest for MockSttRequest {
        fn scripted(run_for: Duration, then_panic: bool) -> Self {
            Self::Scripted {
                run_for,
                then_panic,
            }
        }
    }

    impl Default for MockSttRequest {
        /// Required by `ConformanceRequest: Default`; never actually run
        /// with this value (the battery always builds requests via
        /// `scripted`), so any variant works.
        fn default() -> Self {
            Self::Echo(0)
        }
    }

    #[derive(Debug)]
    struct MockSttResponse {
        sample_count: usize,
        resets_seen: usize,
    }

    impl From<MockSttResponse> for SttResult {
        fn from(r: MockSttResponse) -> Self {
            Self {
                text: format!("{} samples", r.sample_count),
                language: Some("en".to_string()),
                duration_secs: r.sample_count as f32 / 16_000.0,
            }
        }
    }

    impl ene_infer::conformance::ConformanceResponse for MockSttResponse {
        fn resets_seen(&self) -> usize {
            self.resets_seen
        }
    }

    #[derive(Debug, Default)]
    struct MockSttModel {
        resets_seen: usize,
    }

    impl LocalModel for MockSttModel {
        type Request = MockSttRequest;
        type Response = MockSttResponse;
        type Error = MockSttError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "mock-stt"
        }

        fn run(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            match req {
                MockSttRequest::Echo(n) => Ok(MockSttResponse {
                    sample_count: n,
                    resets_seen: self.resets_seen,
                }),
                MockSttRequest::Fail => Err(MockSttError("empty pcm".to_string())),
                MockSttRequest::Slow(run_for) => {
                    let start = Instant::now();
                    loop {
                        if ctx.should_stop().is_some() {
                            return Err(MockSttError("stopped".to_string()));
                        }
                        ctx.tick();
                        if start.elapsed() >= run_for {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(MockSttResponse {
                        sample_count: 999,
                        resets_seen: self.resets_seen,
                    })
                }
                MockSttRequest::Scripted {
                    run_for,
                    then_panic,
                } => {
                    assert!(!then_panic, "scripted panic for conformance testing");
                    let start = Instant::now();
                    loop {
                        if ctx.should_stop().is_some() {
                            return Err(MockSttError("stopped".to_string()));
                        }
                        ctx.tick();
                        if start.elapsed() >= run_for {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(MockSttResponse {
                        sample_count: 0,
                        resets_seen: self.resets_seen,
                    })
                }
            }
        }

        fn reset(&mut self) {
            self.resets_seen += 1;
        }
    }

    fn provider(resource: ResourceClass, cfg: EngineConfig) -> LocalSttEngine<MockSttModel> {
        let handle = ene_infer::EngineHandle::spawn(|| Ok(MockSttModel::default()), cfg);
        let descriptor = EngineDescriptor::new(
            "mock-stt",
            CapabilitySet::empty().with(Capability::Stt),
            resource,
        );
        LocalSttEngine::new(handle, descriptor)
    }

    #[tokio::test]
    async fn transcribe_happy_path() {
        let p = provider(ResourceClass::Gpu { device: 401 }, EngineConfig::default());
        let pcm = vec![0.0_f32; 1600];
        let result = p
            .transcribe(&pcm, 16_000)
            .await
            .expect("transcribe succeeds");
        assert_eq!(result.text, "1600 samples");
        assert_eq!(result.language.as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn transcribe_model_error_maps_to_provider() {
        let p = provider(ResourceClass::Gpu { device: 402 }, EngineConfig::default());
        let err = p
            .transcribe(&[], 16_000)
            .await
            .expect_err("empty pcm is scripted to fail");
        assert!(matches!(err, AudioProviderError::Provider(_)));
    }

    #[tokio::test]
    async fn transcribe_deadline_maps_to_timeout() {
        let p = provider(
            ResourceClass::Gpu { device: 403 },
            EngineConfig::new(4, Duration::from_millis(20)),
        );
        let pcm = vec![0.0_f32; 999];
        let err = p
            .transcribe(&pcm, 16_000)
            .await
            .expect_err("job exceeding job_timeout must surface Timeout");
        assert!(matches!(err, AudioProviderError::Timeout));
    }

    #[test]
    fn map_engine_error_preserves_variant() {
        use ene_infer::EngineError;

        let busy: EngineError<MockSttError> = EngineError::Busy { queue_depth: 1 };
        assert!(matches!(
            map_engine_error(busy),
            AudioProviderError::Busy { queue_depth: 1 }
        ));

        let down: EngineError<MockSttError> = EngineError::EngineDown {
            reason: "died".to_string(),
        };
        assert!(matches!(
            map_engine_error(down),
            AudioProviderError::Provider(_)
        ));
    }

    /// Runs the generic queueing/cancellation/panic-recovery/reset battery
    /// against `MockSttModel` itself (the `LocalModel`, not the
    /// `LocalSttEngine` adapter around it) — this is what keeps this
    /// blanket adapter's assumptions about `ene_infer::LocalModel` correct
    /// as new `LocalModel` implementations (voice engines, future
    /// providers) land beside it.
    #[tokio::test]
    async fn mock_stt_model_passes_conformance_battery() {
        ene_infer::conformance::run_all(MockSttModel::default).await;
    }
}
