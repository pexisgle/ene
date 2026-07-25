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

During conversation turns, `ene-mind` emits `EneEvent::Performance` cues containing avatar expressions:

```rust
pub struct PerformanceCue {
    pub expression: ExpressionKind, // Happy, Angry, Surprised, Neutral, etc.
    pub blink: bool,
    pub viseme: Option<VisemeCategory>, // A, I, U, E, O lip shapes
    pub motion: Option<MotionPreset>,
}
```

`ene-desktop` receives these performance events and maps them directly to VRM blendshapes and skeletal bone animations.
