# Local Voice Pipeline & VRM 3D Avatar Rendering

This document explains Ene's local audio processing engine (`ene-voice`) and 3D avatar rendering system (`ene-vrm`).

---

## 1. Local Voice Pipeline (`ene-voice`)

Ene provides an optional, fully local voice interaction pipeline:

```text
Microphone Input
  │
  ├─> 1. Audio Recording (cpal stream capture)
  ├─> 2. Voice Activity Detection (Silero VAD via ONNX Runtime / ort)
  ├─> 3. Speech-to-Text (Local GGUF Whisper model via whisper-rs)
  │
  └─> LLM Turn Processing
        │
        ├─> 4. Text-to-Speech Generation (TTS engine)
        └─> 5. Audio Playback & Viseme Lip-Sync (rodio)
```

### Components
- **STT (Speech-to-Text)**: Local inference over Whisper models using `whisper-rs`.
- **VAD (Voice Activity Detection)**: Real-time speech segmentation using Silero VAD running on ONNX Runtime (`ort`).
- **TTS (Text-to-Speech)**: Local speech synthesis emitting PCM audio buffers for playback.
- **Audio Devices**: Low-latency PCM stream capture and output via `cpal` and `rodio`.

---

## 2. VRM 3D Avatar Rendering (`ene-vrm`)

`ene-vrm` is a standalone wgpu renderer designed to display VRM 1.0 3D models inside `ene-desktop`.

### Architectural Independence
`ene-vrm` has **zero dependencies** on `ene-mind`, `ene-runtime`, or `ene-store`. It accepts raw geometric meshes, textures, bone transforms, and blendshape weights.

### Performance Cue Mapping

During conversation turns, `ene-runtime` broadcasts `EneEvent::Performance { turn, origin, cues, source }` on the chat bus, where `cues: Vec<ene_mind::PerformanceCue>` comes from `ene-mind`'s output arbitration. Each `PerformanceCue` carries a `kind` (expression / motion / look-at / cancel), a `name` identifying the specific cue, and kind-specific fields (target weight and hold duration for expressions, a `MotionLayer` for motions) — see `cargo doc -p ene-mind --open` (`output::performance::PerformanceCue`) for the authoritative fields.

### Expression Resolution

The final expression of a turn is chosen by `ene-mind`'s expression arbiter from three sources, in priority order:

1. **LLM proposal** — the chat model's `<|perf:expr=…|>` marker (PHI mode) or the affect classifier's `recommended_expression` (emotion mode). Both receive the card's expression list in the prompt and must pick an exact name from it; anything else falls back to neutral — no fuzzy string matching is applied.
2. **Affect mapping** — when no LLM proposal arrived, the arbiter maps the current affect state to the *nearest annotated expression* in affect space. The annotation is defined per expression on the card under `extensions.ene.expressions[].affect`:

   ```json
   {
     "name": "にっこり",
     "vrm": { "happy": 1.0 },
     "affect": { "valence": 0.6, "arousal": 0.3 }
   }
   ```

   The distance is computed over `valence`, `arousal`, `irritation`, and `fatigue` (missing dimensions default to `0.0`). Expressions without an `affect` annotation are never selected by this path. The built-in defaults carry annotations equivalent to the legacy threshold mapping, so cards without `extensions.expressions` keep their previous behaviour.
3. **Neutral fallback** — no annotated expression available (or the expression list is empty).

`ene-desktop` receives these performance events and maps them to VRM blendshapes and skeletal bone animations.
