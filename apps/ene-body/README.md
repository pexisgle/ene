# `ene-body`

VRM overlay **child process**. Host-spawned `ene-desktop` is the parent.
This process does not speak Host, Client `ene-api`, control, secrets, conversation
text, or Task commands. Crate name `ene-vrm` is not used.

Slice D contains production candidates for KDE Wayland layer-shell and Windows
DWM overlays, a real wgpu surface renderer, and a Body-local `vrm-runtime`
0.1 pipeline. The generated VRM fixture probe is automated. Real KDE/Windows
desktop probes and the official `ene` asset acceptance are still **未実施**;
compiled code and headless CI are not acceptance evidence.

## Process contract

- hide / exit are local overlay facts. They are not Companion stop and not Task cancel.
- Crash or hang here must not take chat down. Isolation evidence in this crate is:
  a dummy parent sees disconnect and keeps running. Full GUI survival is desktop's job.
- Restart replays the last projection (show / box / pose / asset). It does not replay domain commands.

## Projection IPC (desktop must match)

Transport is not `ene-plugin-ipc` and not the Host Client protocol. Parent passes a
socketpair / anonymous pipe later. This binary accepts:

| Flag | Meaning |
| --- | --- |
| `--ipc-stdio` (default) | parent→body on stdin, body→parent on stdout |
| `--ipc-fd FD` (Unix) | exclusive inherited connected Unix socket (socketpair end). FDs 0–2 are rejected |
| `--ipc-unix PATH` (Unix) | `connect(2)` to an existing Unix domain socket the parent is listening on |

Stdout is the IPC write side when using stdio. Diagnostics go to stderr only.

### Frame layout

Length-prefixed MessagePack. Same layout in both directions; the payload type
depends on who writes.

```text
offset 0  u32 big-endian body length (exclusive of this prefix)
offset 4  MessagePack body, at most 65536 bytes
```

- Encoding: `rmp_serde::to_vec_named` / `rmp_serde::from_slice` (structs as maps).
- Enums: serde externally tagged (`{"Show":null}`, `{"PoseHint":"Idle"}`, …).
- Unknown variants and unknown struct fields are rejected. The process stays up.
- Asset **bytes are not in the frame**. Only a filesystem path or a parent-owned temp file path.
  Host internal PKs are not a legal ref.

`ParentToBody` (desktop → body):

- `Show` / `Hide`
- `Placement { x, y, width, height, scale }` (`scale` HiDPI, finite and `> 0`; `width`/`height` `> 0`)
- `PoseHint`: `Idle` / `Listening` / `Speaking` / `Working` / `Attention` (no joint angles, no blendshapes)
- `AssetRef::Path { path }` or `AssetRef::BytesTemp { path }`
- `Shutdown`

`BodyToParent` (body → desktop):

- `Ready { overlay, gpu, expressions, spring_bone }`
- `GpuFail { reason }` after Ready when wgpu adapter/device/surface operation failed
- `OverlayUnavailable { requested, reason }` when native overlay creation fails; Headless is not success
- `AssetReady { primitives, expressions, spring_chains }` only after strict VRM/runtime validation
- `AssetFail { reason, detail }` for a missing, invalid, or incomplete VRM (process stays up)
- `HealthTick { seq, visible, pose, gpu_ok, overlay, expressions, spring_bone }` at 4 Hz (not a present-FPS proof)
- `LocalUi`: `Drag { x, y }` / `Resize { width, height }` / `Hide` (overlay-local; not a Host write)
- `Presentation`: a correlated Wayland surface submission followed by `Presented`, `Discarded`, or `Missing`; frame callbacks are never FPS evidence
- `CleanExit` then process exit 0. A kill/abort is a disconnect without `CleanExit`.

## Modules

| Module | Role now | Probe |
| --- | --- | --- |
| `ipc` | Framing and message types | n/a (headless tests cover this) |
| `window` | KDE `zwlr_layer_shell_v1` overlay + input region; Windows layered/DWM popup + non-rectangular hit-test; explicit Headless unavailability | source/build gates pass; real desktop probe **未実施** |
| `render` | wgpu surface, FIFO present, transparent clear, unlit CPU-baked fallback; Body process only | real surface probe **未実施** |
| `vrm` | strict VRM 1.0 load; humanoid pose hints, expression, LookAt, SpringBone at about 30 Hz; CPU renderer frame data | generated fixture passes; official asset pending #1651 |

## Asset runtime probe

`ene-body-asset-probe` is a machine-readable runtime-only probe for a supplied
VRM. It executes every `PoseHint` for one second and rejects missing expression,
LookAt, SpringBone, renderer primitives, or non-finite/empty frame data.

```sh
cargo run -p ene-body --bin ene-body-asset-probe -- /path/to/ene.vrm
```

Its success is not transparent-window or official-character acceptance.

## 未実施 (do not treat as pass)

- KDE Wayland overlay (`zwlr_layer_shell_v1`, click-through input region) on a real compositor. X11/`DISPLAY` in CI is not that evidence.
- Windows 11 layered/DWM transparent hit-test and PresentMon correlation on real hardware.
- `vrm-runtime` load and visible rendering of the official bundled `ene`; the generated fixture is not #1651.
- Japanese IME (desktop slice B, not this process).
- NixOS 26.11 KDE Wayland final acceptance (slice F).
- Performance Gates (FPS via presented frames, idle CPU including this PID).
