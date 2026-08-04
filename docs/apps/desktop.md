# `ene-desktop` User Guide

`ene-desktop` is the GUI desktop application featuring real-time 3D VRM avatar rendering (`ene-vrm`), local voice synthesis/recognition (`ene-voice`), and live emotion/performance expressions.

---

## Launching Desktop

```bash
# Run Desktop application
cargo run -p ene-desktop
```

---

## Key Features

- **VRM 3D Avatar Window**: Renders VRM 1.0 models using `wgpu` hardware acceleration.
- **Lip-Sync & Expression Animation**: Receives `Performance` events from `ene-mind` to animate facial blendshapes (Happy, Angry, Surprised) and visemes (A, I, U, E, O).
- **Local Voice Pipeline**: Supports real-time microphone input via Silero VAD / Whisper STT and speech synthesis output.
- **Always-on-Top / Transparent Window**: Optional desktop overlay mode for a natural companion experience.
- **Character Card Editor**: Visual `CCv3` editing (identity, personality, scenario, greetings, memory instructions, lorebook, motion catalog) with schema/asset validation, atomic saves with backup, and discard confirmation. See the [Character Card Editor Guide](../guide/character-card-editor.md).
