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
5. **TTS** — Accumulated text is synthesized sentence-by-sentence by Kokoro ONNX; PCM chunks are pushed through an mpsc channel and emitted incrementally as `EneEvent::AudioChunk`.
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

1. `ai.{stt,tts,vad}.model_path` when non-empty (env overrides via `ENE_AI__STT__MODEL_PATH`, `ENE_AI__TTS__MODEL_PATH`, `ENE_AI__VAD__MODEL_PATH`)
2. `ai.{stt,tts,vad}.model` when non-empty
3. Default cache location: `{assets_dir}/models/gguf/{whisper.gguf,kokoro.onnx,silero_vad.onnx}`

Kokoro TTS also requires a `voices.bin` file (resolved via `ai.tts.voices_path` / `ENE_AI__TTS__VOICES_PATH`, falling back to the same cache directory).

ONNX Runtime (used by Kokoro TTS and Silero VAD) is initialized once per process via `ensure_ort_init()`. Set `ai.ort_dylib_path` (or `ENE_AI__ORT_DYLIB_PATH`) to load `libonnxruntime` from an explicit path; when unset, `ort`'s default resolution applies (e.g. `LD_LIBRARY_PATH`).

Weights are **not** bundled or auto-downloaded — place them manually or point the config at existing files.

## Supported providers

| Modality | Provider name | Backend | Sample rate | Notes |
|----------|--------------|---------|-------------|-------|
| STT | `whisper` | whisper.cpp (`whisper-rs`) | 16 kHz mono | Resamples from device rate automatically |
| TTS | `kokoro` | Kokoro ONNX (`ort`) | 24 kHz mono | Text is phonemized by a G2P tokenizer before inference; ~53 voices selectable via `voice`; PCM chunks stream incrementally over an mpsc channel; requires `voices.bin` |
| VAD | `silero` | Silero VAD v5 ONNX (`ort`) | 16 kHz mono | `frame_size()` reports the expected frame (512 samples / 32 ms @ 16 kHz); `process_chunk` returns a `Result`; threshold configurable |

Kokoro's `input_ids` tensor is a flat phoneme vocabulary, not raw text: the G2P tokenizer (`crates/ene-ai/src/g2p.rs`) converts input text into phoneme ids — rule-based phonemization for English and a kana→phoneme table for Japanese (selected via `ai.tts.language`; empty defaults to English). Unknown characters are dropped.

ONNX Runtime is loaded dynamically at runtime (`load-dynamic` feature) — the `libonnxruntime` shared library must be discoverable (e.g. `LD_LIBRARY_PATH`, the desktop app's bundled library, or an explicit `ai.ort_dylib_path`).

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

While TTS audio is playing (`AudioState::tts_playing`), the capture callback applies echo-aware suppression instead of hard-muting the microphone. The VAD energy gate is elevated (speech must exceed roughly 2× the normal silence threshold) so that speaker bleed from the character's own voice is filtered out, while genuinely loud user speech still reaches the VAD. When the VAD detects a speech start during playback, a barge-in event fires and cancels the current turn (see [Barge-in behavior](#barge-in-behavior)). Suppression lifts automatically when playback finishes. Full acoustic echo cancellation is not implemented; the elevated threshold is a pragmatic approximation.

## Barge-in behavior

When the user starts speaking while the character is still responding (or its TTS is still playing):

1. **Cancel** — The desktop sends `EneCommand::Cancel` to the runtime, stopping the LLM stream and TTS synthesis.
2. **Partial history** — `ConversationSession::mark_interrupted` commits the portion of the response that had been produced (and typically spoken) to conversation history, so context is not lost.
3. **Interruption tag** — The memory writer tags candidates from the interrupted turn with `"interrupted"`, enabling downstream recall to distinguish complete from partial exchanges.
4. **Next-turn context** — On the following turn, `take_interruption()` injects a system-prompt note acknowledging the interruption, so the model can resume or acknowledge naturally.

This flow is automatic when the microphone is active — no explicit "stop" action is needed from the user.

## Known limitations

- **ggml symbol collision** — `whisper-rs-sys` (STT) and `llama-cpp-sys-2` (local LLM / embedding) each vendor their own copy of ggml, so enabling the voice features produces duplicate-symbol link errors. On Linux this is worked around with `-Wl,--allow-multiple-definition`, scoped to `[target.x86_64-unknown-linux-gnu]` in `.cargo/config.toml`. This can be removed once `whisper-rs-sys` exposes an external-ggml feature so both crates link a single shared ggml.
- **llama-cpp-sys-2 FGDN assertion** — `llama-cpp-sys-2` 0.1.152 still carries a `GGML_ASSERT` that aborts when loading models that omit one of the two FGDN tensor groups (some Gemma variants). `crates/ene-ai/build.rs` patches the cargo-registry source to turn the assert into a skip, gated behind the `patch-llama-fgdn` feature (enabled by default). Once upstream ships the fix, the feature can be dropped from `default` and the build script removed.

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
