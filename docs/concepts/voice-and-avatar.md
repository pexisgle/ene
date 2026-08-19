# Voice & avatar

## Voice

`ene-body` owns duplex voice state (idle / listening / thinking /
responding / speaking / interrupting), energy VAD, and lip-sync visemes.
TTS and STT bind through `ai.tasks.tts` / `ai.tasks.stt` to provider plugins
(`provider.openai_compat`, `provider.elevenlabs`, `provider.voicevox`,
`provider.edge_tts`). PCM is `f32` on the provider subprotocol. Stage owns
the microphone and playback devices. The daemon still owns policy and the live
bus; exclusive resources (mic) are claimed through the API.

Lip-sync maps PCM energy to the same viseme targets `ene-vrm` expects.
Affect in `ene-companion` picks expression cues that `ene-body` queues as
performance commands.

## Performance commands

`ene-body::PerformanceCommand` is what stage consumes:

| Command | Meaning |
|---|---|
| `Expression` | Blend-shape expression |
| `Motion` | Motion clip (layer + intensity) |
| `LookAt` | Gaze target |
| `LipSync` | Mouth weights for the current audio frame |
| `Posture` / vitality | Idle autonomy |

Cues are rate-limited so the avatar does not flicker.

## The avatar (VRM)

`ene-stage` renders **VRM 1.0** with `ene-vrm` (wgpu):

- **Model loading** — `.vrm` from the character package
- **Motions** — VRMA clips with blending layers
- **Expressions** — VRM blend shapes plus procedural blink / gaze / mouth
- **Look-at** — cursor tracking within configured ranges
- **Spring bones** — hair / cloth

The supported rendering API is documented in
[ene-vrm API reference](../reference/api/ene-vrm.md).
