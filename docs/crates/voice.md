# `ene-voice`

> **Crate**: `ene-voice` | **Role**: Local STT / TTS / VAD engine implementations

`ene-voice` encapsulates local speech-to-text (Whisper via `whisper-rs`), text-to-speech (Kokoro ONNX), and voice activity detection (Silero VAD via ONNX Runtime). Since the provider-plugin epic it is consumed exclusively by provider plugin binaries (`plugins/provider/{kokoro,onnx,whisper}`); `ene-runtime` and `ene-desktop` no longer depend on it.

---

## Architectural boundaries

- `ene-voice` depends on `ene-ai` (for provider-adjacent types) and `ene-config`; it has no dependency on `ene-mind`, `ene-runtime`, or `ene-store`. It exposes no in-process provider registry (`ene_ai::AudioProviderRegistry` has been removed) — the provider plugins bridge the engines to the host over the plugin IPC.
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
