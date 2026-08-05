# Voice & avatar

## Voice pipeline

The voice pipeline has three stages, each backed by a provider plugin:

| Stage | Built-in providers | Config |
|---|---|---|
| **STT** (speech → text) | `whisper` (local whisper.cpp) | `ai.stt.provider` |
| **TTS** (text → speech) | `kokoro` (local ONNX), `edge-tts`, `elevenlabs`, `openai-tts`, `voicevox` | `ai.tts.provider`, `model`, `voice`, `speed`, `language` |
| **VAD** (voice activity) | `onnx` (Silero), `none` | `ai.vad.provider` |

The desktop app captures microphone audio (cpal), feeds it through the
chosen STT provider, and plays TTS audio chunks (rodio) that stream back
over the runtime's audio channel. Audio providers resolve through the same
plugin host registry as LLM providers.

Speech affects the avatar in real time:

- **Visemes** — audio is analyzed into mouth-shape weights for lip-sync.
- **Emotions** — the current affect state picks expressions (smile,
  surprise, …).
- **Beat sync** — with a beat-sync device configured, avatar motion pulses
  to the music's BPM.

## Performance cues

During a turn, the mind's output arbiter emits **performance cues**:

| Cue kind | Meaning |
|---|---|
| `expression` | Blend-shape expression to show |
| `motion` | Motion clip to play (with layer + intensity) |
| `lookat` | Where the avatar should look |
| `cancel` | Stop the current performance |

Each cue carries a source (`affect` — from the emotion engine, or `llm` —
requested by the model). Cues are validated before presentation
(expressions must exist on the model, motions must exist in the catalog)
and rate-limited/hysteresis-controlled so the avatar does not flicker
between states.

## The avatar (VRM)

The desktop app renders **VRM 1.0** models with `ene-vrm`, a standalone
wgpu renderer:

- **Model loading** — `.vrm` files from the card's `assets` (or a CLI
  argument), including humanoid bones, expressions, spring bones, node
  constraints, look-at settings, and MToon materials.
- **Motions** — VRMA animation clips (idle, wave, …) with blending layers
  and retargeting.
- **Expressions** — VRM blend shapes composed with procedural overrides
  (blink, gaze, mouth).
- **Look-at** — the avatar tracks the cursor within configured ranges.
- **Spring bones & physics** — hair/cloth simulation, plus scene physics
  for drag interactions.

The supported rendering API is documented in
[ene-vrm API reference](../reference/api/ene-vrm.md).

## Desktop voice/avatar settings

- Settings → Voice: TTS/STT/VAD selection, model/voice pickers, speed.
- Settings → Graphics: quality preset (affects render resolution).
- Settings → Character: default expression/motion, look-at strength,
  position, scale.
- `desktop.caption_enabled` — caption overlay for spoken lines;
  `desktop.beat_sync` — music input for beat-synced motion.

Building without the native audio stack is possible with
`--no-default-features` (voice disabled; the audio module compiles to inert
stubs).
