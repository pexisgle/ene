# `ene-vrm` — API Reference

> **Crate**: `ene-vrm` | **Role**: VRM 1.0 3D model loader & wgpu renderer for `ene-desktop`

`ene-vrm` is a dedicated loader and renderer for VRM 1.0 files, powered by `gltf` and `wgpu`.

---

## Architectural Guarantees
- Zero dependencies on `ene-mind`, `ene-runtime`, or `ene-store`.
- Pure graphics rendering engine taking raw geometry, textures, bone transforms, and blendshape weights.

---

## Core API

```rust
pub struct VrmModel { /* ... */ }

impl VrmModel {
    /// Loads a VRM 1.0 model from a binary GLTF (.vrm) buffer.
    pub fn load_from_bytes(bytes: &[u8]) -> Result<Self, VrmError>;

    /// Updates skeletal bone transforms and blendshape expression weights.
    pub fn update_pose(&mut self, pose: &VrmPose);

    /// Renders the model using the provided wgpu RenderPass.
    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>);
}
```

---

## Related Links
- [Voice & Avatar Concepts](../concepts/voice-and-avatar.md)
- [Desktop Application Guide](../apps/desktop.md)
