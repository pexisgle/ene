# `ene-vrm`

> **Crate**: `ene-vrm` | **Role**: VRM 1.0 3D model loader & wgpu renderer for `ene-desktop`

`ene-vrm` is a dedicated loader and renderer for VRM 1.0 avatar files, built on `gltf` and `wgpu`. It is a pure graphics component: it accepts raw geometry, textures, bone transforms, and blendshape weights, and has no awareness of cognition, memory, or the runtime turn loop.

---

## Architectural boundaries

- `ene-vrm` has zero dependencies on `ene-mind`, `ene-runtime`, or `ene-store`.
- Mapping from a conversation turn's performance cues (expression/motion) to concrete blendshape weights and bone animations happens in `ene-desktop`, not in `ene-vrm` — this crate only renders whatever pose/weights it is given.

## Design rationale

- **Why rendering is decoupled from cognition/runtime**: it lets the avatar renderer be tested, profiled, and evolved (e.g. swapped for a different rendering backend) independently of the chat/turn pipeline, and keeps `wgpu`/graphics dependencies out of crates that don't need them.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-vrm --open
```

---

## Related
- [Voice & Avatar Concepts](../concepts/voice-and-avatar.md)
- [Desktop Application Guide](../apps/desktop.md)
