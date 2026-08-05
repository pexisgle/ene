//! Local text-to-speech provider backed by Kokoro ONNX (`ort`).
//!
//! This module is layered into two compilation surfaces, split structurally
//! rather than threaded through individual functions via scattered
//! `#[cfg(feature = "local-tts")]`:
//!
//! - [`download`] — feature-independent model download and path resolution.
//!   Compiles and can be called with `local-tts` disabled (the kokoro plugin
//!   calls it from its own process before loading the model).
//! - [`provider`] — the `local-tts`-gated ONNX session and
//!   [`ene_ai::TtsProvider`] implementation ([`provider::KokoroModel`]).

/// Feature-independent download and path-resolution layer.
pub mod download;
/// `local-tts`-gated Kokoro ONNX provider layer.
#[cfg(feature = "local-tts")]
pub mod provider;

pub use download::{
    default_kokoro_model_path, default_kokoro_voices_path, ensure_kokoro_files_exist,
};
#[cfg(feature = "local-tts")]
pub use provider::{KokoroError, KokoroModel};

/// Provider name used in `ai.tts.provider` configuration.
pub const PROVIDER_NAME: &str = "kokoro";

/// Runs `ene_infer::conformance::run_all` against a test-only stand-in for
/// [`provider::KokoroModel`]. See `local_stt.rs`'s `conformance_tests`
/// module for why this cannot be the real `KokoroModel` (orphan rule; "run
/// for approximately this long" has no meaningful encoding as synthesis
/// text) and what this battery does and does not validate about the real
/// engine.
///
/// Lives here rather than under `provider` because it depends on neither
/// `ort` nor the `local-tts` feature — it only exercises the
/// [`ene_infer::LocalModel`] wiring pattern that engine would use.
#[cfg(test)]
mod conformance_tests {
    use std::time::{Duration, Instant};

    use ene_infer::conformance::{ConformanceRequest, ConformanceResponse};
    use ene_infer::{JobContext, LocalModel};

    #[derive(Debug, Clone, Default)]
    struct ScriptedTtsRequest {
        run_for: Duration,
        then_panic: bool,
    }

    impl ConformanceRequest for ScriptedTtsRequest {
        fn scripted(run_for: Duration, then_panic: bool) -> Self {
            Self {
                run_for,
                then_panic,
            }
        }
    }

    #[derive(Debug)]
    struct ScriptedTtsResponse {
        resets_seen: usize,
    }

    impl ConformanceResponse for ScriptedTtsResponse {
        fn resets_seen(&self) -> usize {
            self.resets_seen
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("scripted kokoro stand-in stopped cooperatively")]
    struct ScriptedTtsError;

    #[derive(Debug, Default)]
    struct ScriptedTtsModel {
        resets_seen: usize,
    }

    impl LocalModel for ScriptedTtsModel {
        type Request = ScriptedTtsRequest;
        type Response = ScriptedTtsResponse;
        type Error = ScriptedTtsError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "scripted-kokoro"
        }

        fn run(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            assert!(!req.then_panic, "scripted panic for conformance testing");
            let start = Instant::now();
            loop {
                if ctx.should_stop().is_some() {
                    return Err(ScriptedTtsError);
                }
                ctx.tick();
                if start.elapsed() >= req.run_for {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(ScriptedTtsResponse {
                resets_seen: self.resets_seen,
            })
        }

        fn reset(&mut self) {
            self.resets_seen += 1;
        }
    }

    #[tokio::test]
    async fn kokoro_engine_wiring_passes_conformance_battery() {
        ene_infer::conformance::run_all(ScriptedTtsModel::default).await;
    }
}
