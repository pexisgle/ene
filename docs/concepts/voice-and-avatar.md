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
- **Concurrent bodies** — the overlay composites up to
  `body.render.max_concurrent` meshes (default 2). Visemes apply to the
  active soul; A/D retargets the chat session without unloading the other body.

The supported rendering API is documented in
[ene-vrm API reference](../reference/api/ene-vrm.md).

## VRMA playback

Stage auto-plays an idle clip when motions are present: a file named `idle`,
else a name containing `VRMA_01`, else the first discovered `.vrma`. Changing
clips resets the rest pose and spring bones.

`ene-vrm` samples VRMA with `evaluate_retargeted`: NormalizedLocalRotation
(NLR) onto the destination humanoid rest pose, and hips translation as
destination **local** glTF:

`dst_rest_local + (src_pose - src_rest_local) * (dst_global_y / src_global_y)`.

VRMA translation channels are absolute local values, not world deltas. Adding
them onto rest world Y doubles hips height and clips the model. The overlay
then locks hips **XZ** to the destination rest (keeps Y) so walk cycles stay
on screen. Look-at still applies after VRMA. VRoid models already face `+Z`;
there is no 180° Y camera flip.
