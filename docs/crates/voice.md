# `ene-voice`

> **Crate**: `ene-voice` | **Role**: Local audio STT, TTS, VAD, and cross-platform PCM device I/O

`ene-voice` encapsulates local speech-to-text (Whisper via `whisper-rs`), text-to-speech, voice activity detection (Silero VAD via ONNX Runtime), and cross-platform audio device streams (`cpal`/`rodio`). It is consumed directly by `apps/ene-desktop`, not by `ene-runtime`.

---

## Architectural boundaries

- `ene-voice` depends on `ene-ai` (for provider-adjacent types) and `ene-config`; it has no dependency on `ene-mind`, `ene-runtime`, or `ene-store` — voice I/O is a presentation-layer concern the host app wires up around the chat turn, not something the cognitive/runtime layers need to know about.
- All inference here (Whisper transcription, Silero VAD) runs locally and in-process; there is no network call in the voice pipeline itself.

## Design rationale

- **Why local-only inference**: voice capture and playback are latency-sensitive and privacy-sensitive by nature (raw microphone audio); routing them through local models avoids both the round-trip latency and the data-exposure surface of a remote STT/TTS API.
- **Why VAD is a separate stage from STT**: running full Whisper transcription continuously would be wasteful; Silero VAD cheaply detects speech start/stop boundaries so transcription only runs on actual utterances.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-voice --open
```

---

## Related
- [Voice & Avatar Concepts](../concepts/voice-and-avatar.md)
- [System Architecture](../architecture.md)
