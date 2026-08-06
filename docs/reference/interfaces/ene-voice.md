# `ene-voice` interface

## Role

Local voice-pipeline engines: STT (Whisper), TTS (Kokoro), and VAD (Silero).
Consumed by provider plugin binaries over the plugin IPC; this crate is the
engine layer, not the provider.

## Public modules

| Module | Gate | Contents |
|---|---|---|
| `local_stt` | `local-stt` | Whisper STT engine (`LocalModel` impl) |
| `local_tts` | `local-tts` | Kokoro TTS engine + model/voices file management (`default_kokoro_model_path`, `default_kokoro_voices_path`, `ensure_kokoro_files_exist`) |
| `silero_vad` | `silero-vad` | Silero VAD engine |
| `g2p` | — | Grapheme-to-phoneme conversion for Kokoro |
| `ort_init` | any ort feature | Shared ONNX Runtime initializer |

## Dependencies

- Depends on: `ene-ai`, `ene-config`, `ene-infer` (+ optional native
  runtimes: `whisper-rs`, `ort`).
- Used by: `plugins/provider/whisper`, `plugins/provider/kokoro`,
  `plugins/provider/onnx` (Silero VAD).

## Refactoring notes

- Engines here are **`LocalModel` implementations** — the single-threaded
  worker discipline comes from `ene-infer`; do not introduce
  provider-owned concurrency around them.
- Feature flags (`local-stt` / `local-tts` / `silero-vad`) keep native
  runtimes out of builds that do not use them. Adding a feature-gated
  engine must keep the gate; the ORT initializer is shared, not per-engine.
- Model files are downloaded to `assets/models/` (gitignored) on first use;
  path defaults are part of the interface for the provider plugins.
