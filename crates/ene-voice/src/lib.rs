//! Local voice pipeline providers (STT / TTS / VAD) for the ene AI character platform.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit/integration tests use expect for assertions"
    )
)]

/// Grapheme-to-phoneme conversion for Kokoro TTS (C4).
pub mod g2p;
/// Local STT (whisper.cpp) provider.
pub mod local_stt;
/// Local TTS (Kokoro ONNX) provider.
pub mod local_tts;
/// Shared ONNX Runtime initializer.
pub mod ort_init;
/// Silero VAD engine.
pub mod silero_vad;

pub use local_tts::{
    default_kokoro_model_path, default_kokoro_voices_path, ensure_kokoro_files_exist,
};
