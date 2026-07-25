# `ene-voice` — API Reference

> **Crate**: `ene-voice` | **Role**: Local audio STT, TTS, VAD, and cpal PCM device I/O

`ene-voice` encapsulates local speech-to-text (Whisper), text-to-speech, voice activity detection (Silero VAD), and cross-platform audio device streams (`cpal`/`rodio`).

---

## Core Components & API

### Speech-To-Text (`SttEngine`)
Uses `whisper-rs` to perform local GGUF Whisper transcription on captured PCM audio:

```rust
pub struct SttEngine { /* ... */ }

impl SttEngine {
    pub async fn transcribe(&self, pcm_samples: &[f32]) -> Result<String, VoiceError>;
}
```

### Voice Activity Detection (`SileroVad`)
Uses ONNX Runtime (`ort`) to detect user speech start and stop boundaries in real time:

```rust
pub struct SileroVad { /* ... */ }

impl SileroVad {
    pub fn is_speech(&mut self, chunk: &[f32]) -> Result<bool, VoiceError>;
}
```

### Text-To-Speech (`TtsEngine`)
Synthesizes speech text into PCM audio buffers for playback and viseme lip-sync calculations.

---

## Related Links
- [Voice & Avatar Concepts](../concepts/voice-and-avatar.md)
- [System Architecture](../architecture.md)
