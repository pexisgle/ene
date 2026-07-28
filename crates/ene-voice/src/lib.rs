//! Local voice pipeline providers (STT / TTS / VAD) for the ene AI character platform.
//!
//! `clippy::expect_used` is opted out of per test module (not crate-wide):
//! only the `local-tts`-gated tests in `local_tts` actually use `.expect(`,
//! so a crate-level `expect(...)` attribute would go unfulfilled (and fail
//! `-D warnings`) whenever the crate is checked without that feature.
#![warn(missing_docs)]

/// Grapheme-to-phoneme conversion for Kokoro TTS (C4).
pub mod g2p;
/// Local STT (whisper.cpp) provider.
pub mod local_stt;
/// Local TTS (Kokoro ONNX) provider.
pub mod local_tts;
/// Shared ONNX Runtime initializer.
///
/// Not compiled without at least one of these features: the module is a thin
/// wrapper around the optional `ort` dependency, which those features gate.
#[cfg(any(feature = "local-tts", feature = "silero-vad"))]
pub mod ort_init;
/// Silero VAD engine.
pub mod silero_vad;

pub use local_tts::{
    default_kokoro_model_path, default_kokoro_voices_path, ensure_kokoro_files_exist,
    prefetch_if_configured,
};

/// Register this crate's local STT / TTS / VAD provider factories with
/// [`ene_ai::AudioProviderRegistry`].
///
/// Must be called once during host startup, before anything resolves a
/// `"whisper"` / `"kokoro"` / `"silero"` provider by name (see
/// `ene_runtime::handle::EneHandle::open`, which calls this immediately
/// after constructing its command channels). Previously each provider
/// registered itself via a `#[ctor::ctor]` that ran before `main` — before
/// `tracing` was initialized, so a registration failure (there isn't one
/// today; `Arc::new(factory)` cannot fail) would have been invisible. Calling
/// this explicitly from bootstrap, after `tracing` is up, gives registration
/// an observable place to log, should that ever change.
///
/// Idempotent to call more than once: each factory is simply reinserted into
/// the registry's map.
pub fn register_providers() {
    local_stt::register();
    local_tts::register();
    silero_vad::register();
}
