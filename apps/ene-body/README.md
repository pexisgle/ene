# `ene-body`

VRM overlay **child process**. Host-spawned `ene-desktop` is the parent.
This process does not speak Host, Client `ene-api`, control, secrets, conversation
text, or Task commands. Crate name `ene-vrm` is not used.

Slice D renderer-core: projection IPC, a headless overlay, a wgpu device
attempt, and a VRM loader boundary. OS overlay and VRM runtime probes are
**未実施** — this tree must not be read as Wayland / DWM / SpringBone / 26.11
acceptance.

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
- `GpuFail { reason }` after Ready when wgpu adapter/device init failed or timed out
- `AssetFail { reason }` for a missing/unreadable ref (process stays up)
- `HealthTick { seq, visible, pose, gpu_ok, overlay, expressions, spring_bone }` at 4 Hz (not a present-FPS proof)
- `LocalUi`: `Drag { x, y }` / `Resize { width, height }` / `Hide` (overlay-local; not a Host write)
- `CleanExit` then process exit 0. A kill/abort is a disconnect without `CleanExit`.

## Modules

| Module | Role now | Probe |
| --- | --- | --- |
| `ipc` | Framing and message types | n/a (headless tests cover this) |
| `window` | Headless placement/visibility. KDE `zwlr_layer_shell_v1` + input region and Windows layered/DWM are compiled stubs | overlay **未実施** |
| `render` | wgpu instance/adapter/device in **this process only**. No OS surface while overlay is stubbed. No present busy-loop | GPU overlay **未実施** |
| `vrm` | Loader boundary. `vrm-runtime` is the provisional preferred runtime and is **not** locked as adopted (0.1 API; no VRM/SpringBone probe) | expression / SpringBone **Unsupported** |

## 未実施 (do not treat as pass)

- KDE Wayland overlay (`zwlr_layer_shell_v1`, click-through input region). X11/`DISPLAY` in CI is not that evidence.
- Windows 11 layered/DWM transparent hit-test.
- `vrm-runtime` load of bundled `ene`, idle/speaking expression, SpringBone.
- Japanese IME (desktop slice B, not this process).
- NixOS 26.11 KDE Wayland final acceptance (slice F).
- Performance Gates (FPS via presented frames, idle CPU including this PID).
