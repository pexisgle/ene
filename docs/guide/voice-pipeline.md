# Voice pipeline

The voice pipeline enables spoken conversation with your character: microphone input is transcribed to text, the response is synthesized to speech, and the character's mouth moves in sync.

[← Guide index](index.md) · [日本語](../ja/guide/voice-pipeline.md)

## Architecture overview

```
Mic (cpal) → VAD (Silero) → STT (whisper.cpp) → EneHandle::run(text)
                                                        │
                                                        ▼
                                            LLM streaming (TextDelta)
                                                        │
                                                        ▼
                                            TTS (Kokoro ONNX) → AudioChunk events
                                                        │
                                          ┌─────────────┼─────────────┐
                                          ▼             ▼             ▼
                                     rodio playback  VisemeAnalyzer  Self-voice
                                     (speakers)     (lip-sync)      suppression
```

**Data flow:**

1. **Capture** — `cpal` captures microphone audio and feeds it to the VAD engine.
2. **VAD** — Silero VAD detects speech start/end boundaries.
3. **STT** — On speech end, the accumulated PCM is transcribed by whisper.cpp and submitted as a text turn via `EneHandle::run`.
4. **LLM** — The mind streaming pipeline generates the response as `TextDelta` events.
5. **TTS** — Accumulated text is synthesized sentence-by-sentence by Kokoro ONNX; each chunk is emitted as `EneEvent::AudioChunk`.
6. **Playback** — `rodio` plays the PCM audio through the default output device.
7. **Viseme** — The same PCM drives `VisemeAnalyzer` (in `ene-vrm`), which computes mouth-shape weights (`aa`/`ih`/`ou`/`ee`/`oh`) applied to the VRM expression layer each render frame.

## Enabling voice features

### Cargo features

The desktop app enables voice by default:

```toml
# apps/ene-desktop/Cargo.toml
[features]
default = ["voice"]
voice = ["dep:cpal", "dep:rodio"]
```

Build a text-only shell without the native audio toolchain:

```bash
rtk cargo build -p ene-desktop --no-default-features
```

The `ene-ai` crate gates each local provider behind its own feature:

| Feature | Provider | Native dependency |
|---------|----------|-------------------|
| `local-stt` | whisper.cpp STT | `whisper-rs` |
| `local-tts` | Kokoro ONNX TTS | `ort` (ONNX Runtime) |
| `silero-vad` | Silero VAD | `ort` (ONNX Runtime) |
| `voice` | All of the above | — |

### Configuration

Set the provider names in `settings.json` (or via environment variables):

```json
{
  "ai": {
    "tts": { "provider": "kokoro", "voice": "af_heart", "speed": 1.0 },
    "stt": { "provider": "whisper", "language": "ja" },
    "vad": { "provider": "silero", "threshold": 0.5 }
  }
}
```

All providers default to `"none"` (disabled). See [Configuration reference](../reference/configuration/settings.md#ai-tts--text-to-speech) for the full field list.

### Model files

Local providers require model weights on disk. Resolution order for each:

1. Explicit env var (`ENE_AI__STT__MODEL_PATH`, `ENE_AI__TTS__MODEL_PATH`, `ENE_AI__VAD__MODEL_PATH`)
2. `ai.{stt,tts,vad}.model` when it looks like a filesystem path
3. Default cache location: `{assets_dir}/models/gguf/{whisper.gguf,kokoro.onnx,silero_vad.onnx}`

Kokoro TTS also requires a `voices.bin` file (resolved via `ENE_AI__TTS__VOICES_PATH` or the same cache directory).

Weights are **not** bundled or auto-downloaded — place them manually or point the config at existing files.

## Supported providers

| Modality | Provider name | Backend | Sample rate | Notes |
|----------|--------------|---------|-------------|-------|
| STT | `whisper` | whisper.cpp (`whisper-rs`) | 16 kHz mono | Resamples from device rate automatically |
| TTS | `kokoro` | Kokoro ONNX (`ort`) | 24 kHz mono | Streams ~0.25 s chunks; requires `voices.bin` |
| VAD | `silero` | Silero VAD v5 ONNX (`ort`) | 16 kHz mono | 512-sample (32 ms) chunks; threshold configurable |

ONNX Runtime is loaded dynamically at runtime (`load-dynamic` feature) — the `libonnxruntime` shared library must be discoverable (e.g. `LD_LIBRARY_PATH` or the desktop app's bundled library).

## Desktop app usage

### Microphone button

The chat UI includes a microphone toggle button. Clicking it:

1. Resolves STT and VAD providers from the current `AiConfig`.
2. Opens the selected (or default) input device via `cpal`.
3. Starts streaming audio through VAD → STT → `AiBridge::run`.

Click again to stop capture. If STT is disabled (`ai.stt.provider = "none"`), the button shows an error.

### Settings (Features tab)

The Features settings page exposes:

- **Microphone device** — device name override (empty = OS default)
- **VAD threshold** — speech probability slider (0.0–1.0)
- **STT / TTS provider** — read-only display of the configured provider

### Self-voice suppression

While TTS audio is playing (`AudioState::tts_playing`), the capture callback discards all microphone input and resets the VAD. This prevents the character's own synthesized voice from being transcribed back into a new turn. Suppression lifts automatically when playback finishes.

## Barge-in behavior

When the user starts speaking while the character is still responding (or its TTS is still playing):

1. **Cancel** — The desktop sends `EneCommand::Cancel` to the runtime, stopping the LLM stream and TTS synthesis.
2. **Partial history** — `ConversationSession::mark_interrupted` commits the portion of the response that had been produced (and typically spoken) to conversation history, so context is not lost.
3. **Interruption tag** — The memory writer tags candidates from the interrupted turn with `"interrupted"`, enabling downstream recall to distinguish complete from partial exchanges.
4. **Next-turn context** — On the following turn, `take_interruption()` injects a system-prompt note acknowledging the interruption, so the model can resume or acknowledge naturally.

This flow is automatic when the microphone is active — no explicit "stop" action is needed from the user.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| Mic button does nothing | `ai.stt.provider = "none"` | Set `ai.stt.provider` to `"whisper"` |
| `audio init error` on startup | Missing model file | Place weights at the expected path or set `ENE_AI__*_MODEL_PATH` |
| ONNX Runtime not found | `libonnxruntime.so` not on library path | Set `LD_LIBRARY_PATH` or install the ONNX Runtime package |
| No lip-sync movement | TTS disabled or `AudioChunk` not consumed | Ensure `ai.tts.provider` is set and the desktop `voice` feature is enabled |
| Character hears itself | Self-voice suppression not active | Check that playback sets `tts_playing` correctly (desktop `voice` feature) |

## Related documentation

- [Streaming Events](../reference/runtime/streaming-events.md#audio-streaming) — `AudioChunk` event reference
- [Configuration](../reference/configuration/settings.md#ai-tts--text-to-speech) — `ai.tts` / `ai.stt` / `ai.vad` fields
- [Proactive speech](proactive-speech.md) — unsolicited companion utterances (separate from voice I/O)
