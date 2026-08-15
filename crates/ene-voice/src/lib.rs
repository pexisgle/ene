//! Local voice pipeline engines (STT / TTS / VAD) for the ene AI character
//! platform.
//!
//! These engines are consumed by provider plugin binaries
//! (`plugins/provider/onnx`, `whisper`, `kokoro`) over the plugin IPC; the
//! former in-process `ene_ai::AudioProviderRegistry` is removed.
//!
//! `clippy::expect_used` is opted out of per test module (not crate-wide):
//! only the `local-tts`-gated tests in `local_tts` actually use `.expect(`,
//! so a crate-level `expect(...)` attribute would go unfulfilled (and fail
//! `-D warnings`) whenever the crate is checked without that feature.

/// Grapheme-to-phoneme conversion for Kokoro TTS.
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
};
