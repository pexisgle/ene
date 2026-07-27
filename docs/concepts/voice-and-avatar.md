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

`ene-desktop` receives these performance events and maps them to VRM blendshapes and skeletal bone animations.
