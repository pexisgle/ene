
# `ene-vrm` interface

## Role

Standalone VRM 1.0 loader and wgpu renderer for the stage avatar.
Deliberately imports no cognitive, memory, or runtime types.

## Public modules

| Module | Contents |
|---|---|
| `prelude` | Curated **supported API** re-exports — start here |
| `loader` | `load_vrm` |
| `model` | `VrmModel`, `VrmMesh`, `VrmTexture`, `Skeleton`, `NodeHierarchy`, `AlphaMode`, `MeshVertex` |
| `animation` | `VrmaClip`, `VrmaPlayer`, `VrmaFrame`, `evaluate_clip`, retargeting, `Interpolation`, `RepeatMode` |
| `expression` | `ExpressionLayer`, `ExpressionName`, `PrimitiveMorphs`, `MAX_MORPH_TARGETS_PER_PRIMITIVE` |
| `expression_override` | `apply_overrides`, `ExpressionOverrideSettings`, blink/gaze/mouth target names |
| `humanoid` | `HumanoidBoneRegistry`, `VrmBone`, `HUMANOID_BONE_NAMES`, `canonicalize_bone_name` |
| `look_at` | `LookAtEvaluator`, `LookAtProperties`, `LookAtOutput`, range maps |
| `spring_bone` | spring-bone simulation types |
| `node_constraint` | `NodeConstraintRegistry`, constraint types |
| `mtoon` | `MToonMaterial`, textures/uniforms, `OutlineWidthMode` |
| `renderer` | `VrmRenderer` |
| `camera` | `OrthographicCamera`, `ModelUniform`, view-space helpers |
| `viseme` | `VisemeAnalyzer`, `VisemeWeights` (lip-sync) |
| `beat_sync` | `BeatSway` |
| `debug_renderer` | `DebugRenderer`, line/sphere helpers |
| `layer_composer` | `MotionLayer` |
| `error` | `VrmError`, `VrmResult` |

## Dependencies

- Depends on: nothing internal (wgpu, gltf, glam, image, …).
- Used by: `ene-stage` only.

## Refactoring notes

- **Supported API vs internal**: `prelude` and the
  [API reference](../api/ene-vrm.md) name the supported subset. Many
  sub-parsers (`load_humanoid_bones`, `load_look_at`, `load_mtoon_materials`,
  …) are `#[doc(hidden)]` — they are called only from `load_vrm`. Keep
  their visibility or explicitly widen the supported surface first.
- The crate stays decoupled by contract: adding mind/runtime/store
  dependencies here is an architecture violation.
