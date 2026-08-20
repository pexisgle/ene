# ene-vrm API

`ene-vrm` is a platform-agnostic VRM 1.0 loader and renderer built on
`wgpu` 29 and `gltf` 1.4. It is used by `ene-stage`; this page names the
**intentionally supported API** (other `pub` items are callable but not
part of the supported surface).

## Start here: `prelude`

`ene_vrm::prelude` re-exports the curated supported surface. Everything
below is in it unless noted.

## Loading

```rust
let model = ene_vrm::load_vrm(path)?;            // VrmModel
let clip = ene_vrm::load_vrma(path)?;            // VrmaClip (animation)
```

| Item | Purpose |
|---|---|
| `loader::load_vrm` | Parse a `.vrm` file into a `VrmModel` (meshes, skeleton, expressions, spring bones, look-at, materials) |
| `model::VrmModel` / `VrmMesh` / `VrmTexture` | Loaded model data |
| `error::VrmError` / `VrmResult` | Unified error type |

## Animation

| Item | Purpose |
|---|---|
| `VrmaClip` / `VrmaFrame` | A parsed VRMA clip: bone/expression/look-at channels with keyframe interpolation |
| `VrmaPlayer` | Playback: seek, play, repeat modes, blending weight |
| `evaluate_clip` | Sample a clip at a time into bone/expression output |
| `retarget_rotation` / `retarget_hips_translation` | Retargeting helpers |
| `Interpolation` / `RepeatMode` | Channel interpolation and looping |
| `MotionLayer` (in `layer_composer`) | Layer composition for overlapping motions |
| `BeatSway` (`beat_sync`) | Beat-driven sway for music sync |

## Expressions

| Item | Purpose |
|---|---|
| `ExpressionLayer` / `ExpressionName` | Blend-shape layer and standard names |
| `expression_compositor` | Combine multiple expression layers |
| `expression_override` | Procedural overrides: blink, gaze, mouth targets (`apply_overrides`) |
| `viseme` | Audio-driven mouth-shape analysis (lip-sync weights) |

## Skeleton and look-at

| Item | Purpose |
|---|---|
| `HumanoidBoneRegistry` / `VrmBone` | The spec's 55 humanoid bones → glTF nodes; `canonicalize_bone_name` |
| `LookAtEvaluator` / `LookAtProperties` | Per-frame look-at evaluation with range maps |
| `NodeConstraintRegistry` | Node constraints (aim/roll/rotation) |
| `SpringBone` | Spring-bone simulation (hair/cloth) |

## Rendering

| Item | Purpose |
|---|---|
| `renderer` | wgpu render pipeline, bind groups, texture management |
| `camera::OrthographicCamera` | Camera uniform + view-projection helpers (`ndc_to_view_pos`, `pixel_to_ndc`, `view_pos_to_world`) |
| `mtoon::MToonMaterial` | MToon material support (textures, uniforms, outline modes) |
| `post_process` | Post-processing pipeline |
| `debug_renderer` | Debug line/sphere rendering helpers |

## Examples

- `examples/diagnostic_model_matrix.rs` — model-loading diagnostics.
- `examples/inspect_aabb.rs` — bounding-box inspection.

## Scope

- VRM **1.0** only (`.vrm`, `.vrma`).
- Rendering-only: no cognitive, memory, or runtime types are imported.
- Some loaders (`load_humanoid_bones`, `load_look_at`, `load_spring_bones`,
  `load_mtoon_materials`, …) are `#[doc(hidden)]` because they are called
  only from `load_vrm`; their types remain `pub` as part of the model's
  public fields.
