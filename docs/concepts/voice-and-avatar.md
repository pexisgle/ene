# Local Voice Pipeline & VRM 3D Avatar Rendering

This document explains Ene's local voice pipeline (engine code in `ene-voice`, served to the app by the voice provider plugins) and 3D avatar rendering system (`ene-vrm`).

---

## 1. Local Voice Pipeline

The engine implementations (whisper.cpp STT, Kokoro ONNX TTS, Silero VAD)
live in `ene-voice` and run inside the voice provider plugin processes
(`plugins/provider/whisper`, `plugins/provider/kokoro`,
`plugins/provider/onnx`); the host bridges them to the app over the plugin
IPC. The desktop only owns the audio device I/O (cpal capture / rodio
playback).

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

1. **LLM proposal** — the chat model's `<|perf:expr=…|>` marker (PHI mode) or the affect classifier's `recommended_expression` (emotion mode). Both receive the card's expression list in the prompt and must pick an exact name from it (case-insensitive); anything else is rejected — no fuzzy string matching is applied. A rejected proposal falls back to the affect-mapped expression when one exists, otherwise to neutral (or the first listed name when the card has no neutral).
2. **Affect mapping** — when no LLM proposal arrived, the arbiter maps the current affect state to the *nearest annotated expression* in affect space. The annotation is defined per expression on the card under `extensions.ene.expressions[].affect`:

   ```json
   {
     "name": "にっこり",
     "vrm": { "happy": 1.0 },
     "affect": { "valence": 0.6, "arousal": 0.3 }
   }
   ```

   The distance is computed over `valence`, `arousal`, `irritation`, and `fatigue` (missing dimensions default to `0.0`). Expressions without an `affect` annotation are never selected by this path. The built-in defaults carry annotations approximating the legacy threshold mapping, so cards without `extensions.expressions` behave like before for typical states — exact equivalence is impossible because the old priority chain and nearest-neighbour disagree in boundary regions (e.g. high valence with moderate irritation now maps to happy where the old chain returned angry). A neutral state (all dimensions near `0.0`) only matches an annotation near the origin — the card's resting expression; without one, the face falls back to neutral rather than wearing an emotional expression at rest.
3. **Neutral fallback** — no annotated expression available (or the expression list is empty).

`ene-desktop` receives these performance events and maps them to VRM blendshapes and skeletal bone animations.

### Timed mid-utterance expressions (TTS sync)

When TTS is enabled, expression markers change the avatar's face **during** the
utterance, synced to audio playback:

1. Each `<|perf:expr=…|>` marker is tagged with its character offset in the
   spoken (marker-stripped) text (`PerformanceCue::text_offset`).
2. The TTS sentence splitter attributes each marker to the sentence whose text
   range contains it. A marker between two sentences applies to the following
   sentence; a marker that arrives after its sentence was already sent applies
   to the next one; a marker trailing the final text is covered by the
   turn-end event instead.
3. The sentence's cues ride on the first PCM chunk of that sentence
   (`AudioChunk::cues`, dedicated audio channel). The desktop playback path
   switches the expression when that sentence's audio starts playing,
   scheduling the cue on the emotion pipeline, which honors `hold=SECS`
   (default 4 s) and fades the expression out afterwards. The next marker
   replaces the current expression. A sentence without a marker keeps the
   current expression (its hold and fade continue); a reply carrying no
   expression marker at all leaves the expression to the usual turn-end
   resolution.
4. Marker-driven mid-turn switches bypass the end-of-turn hysteresis (they
   come from the marker language, not the affect arbiter); the end-of-turn
   resolution is unchanged.
5. `cancel:expr` stops the timed path: cues not yet attached to a TTS
   sentence are dropped and later expression markers are ignored, so the
   face keeps its current expression for the rest of the utterance. Cues
   already attached to audio chunks still fire, and the currently shown
   expression is not cleared mid-utterance; the cancel lands in the
   turn-end `EneEvent::Performance`, after which the desktop clears its
   scheduled and active expression state.

Motions and look-at cues remain turn-unit: they are applied once at turn end
through `EneEvent::Performance`.

**Without TTS** there is no audio timeline to sync to, so markers keep the
turn-end behavior — accumulated by the runtime and resolved into a single
`EneEvent::Performance` by the expression arbiter. The TTS-on / TTS-off
behaviors differ by design.

---

## 3. Beat Sync: system-audio rhythm & spectrum avatar motion

Beat Sync makes the avatar react to the music or video currently playing on
the system: the desktop captures the audio **output** loopback, detects the
beat in real time, and sways the avatar's body on the rhythm.

### Signal flow

```text
System audio output (monitor loopback)
  │
  ├─> 1. Loopback capture (cpal, `desktop.beat_sync` enabled)
  ├─> 2. FFT + low-frequency energy onset detection (rustfft)
  ├─> 3. { bpm, intensity } pulse → ene-runtime chat bus
  │      (EneEvent::BeatPulse, via EneHandle::report_beat_pulse)
  └─> 4. ene-vrm BeatSway: procedural body sway + VRMA speed sync
```

### Detection algorithm

Energy-based onset detection on the low-frequency ("kick") band:

1. The capture stream is low-pass filtered at ~250 Hz (2nd-order Butterworth)
   so out-of-band content cannot leak into the analysis band.
2. Every ~21 ms a Blackman-Harris-windowed 4096-point FFT is computed; the
   mean magnitude over the ≈20–150 Hz band (DC excluded) is the frame energy.
3. An onset fires when the energy exceeds a slow exponential average by a
   fixed margin, the kick band holds at least 40% of the sub-500 Hz energy
   (suppresses hi-hats and transients), a ~250 ms refractory period has
   elapsed, and an absolute floor is cleared (silence never triggers).
4. Onset intensity is the normalized energy overshoot `1 - avg/energy`
   (clamped to `[0, 1]`); BPM is `60 / median(inter-onset interval)` over the
   recent intervals, clamped to 60–180.

This energy-onset approach was chosen over autocorrelation or comb-filter
tempo trackers for robustness and simplicity at this scope; it tracks
kick-driven music well and degrades to no onsets (stillness) on speech or
quiet audio.

### Platform support

`cpal` has no loopback API: on Linux it enumerates `ALSA`/`PipeWire` capture
devices, so loopback works where a monitor source is exposed as an input
device (`PipeWire` monitor ports; `PulseAudio` monitor sources visible to
`pipewire-alsa`). Device selection order:

1. `desktop.beat_sync.device` — explicit device name override.
2. The input device whose name contains the default **output** device name
   (the `<output>.monitor` convention).
3. Any input device whose name contains "monitor".

When no monitor device exists the feature logs a warning and stays disabled —
it never falls back to the default microphone. Windows is cross-compiled but
unsupported: WASAPI has no monitor-style input enumeration, so the feature
degrades to disabled there.

### Avatar reaction

- **Procedural sway** (`ene-vrm::beat_sync::BeatSway`, no assets required):
  beat-locked rotations on hips / spine / chest / head (≤ ~4°), phase snapped
  to the sine peak on each pulse (max rotation lands on the beat) with an
  intensity envelope that decays between beats.
- **Locomotion speed sync**: while a beat is active, the currently-playing
  VRMA clip's `VrmaPlayer.speed` is scaled by `bpm / 120` (clamped
  0.85–1.2), so walk/dance clips follow the tempo.
- Performance cues (`EneEvent::Performance`) are untouched: sway composes
  *after* VRMA retargeting and *before* the skin palette, additively on top
  of any motion the LLM/affect pipeline selected.

### Configuration

`desktop.beat_sync.enabled` (default `false` — capturing system audio by
default would be a privacy surprise) and `desktop.beat_sync.device` (optional
override). The Features page exposes the enabled toggle; changing it starts
or stops the capture thread without restarting the app. Beat sync requires
the desktop's `voice` build feature (the same cpal/ALSA toolchain as mic
capture and TTS playback).
