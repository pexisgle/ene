# `ene-vrm` インターフェース

## 役割

desktop アバター向けの独立した VRM 1.0 ローダー兼 wgpu レンダラー。
意図的に認知・メモリ・ランタイム型を import しません。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `prelude` | 厳選された**サポート対象 API** の再エクスポート — 最初に見る場所 |
| `loader` | `load_vrm` |
| `model` | `VrmModel`・`VrmMesh`・`VrmTexture`・`Skeleton`・`NodeHierarchy`・`AlphaMode`・`MeshVertex` |
| `animation` | `VrmaClip`・`VrmaPlayer`・`VrmaFrame`・`evaluate_clip`・リターゲット・`Interpolation`・`RepeatMode` |
| `expression` | `ExpressionLayer`・`ExpressionName`・`PrimitiveMorphs`・`MAX_MORPH_TARGETS_PER_PRIMITIVE` |
| `expression_override` | `apply_overrides`・`ExpressionOverrideSettings`・まばたき/視線/口のターゲット名 |
| `humanoid` | `HumanoidBoneRegistry`・`VrmBone`・`HUMANOID_BONE_NAMES`・`canonicalize_bone_name` |
| `look_at` | `LookAtEvaluator`・`LookAtProperties`・`LookAtOutput`・レンジマップ |
| `spring_bone` | spring bone シミュレーション型 |
| `node_constraint` | `NodeConstraintRegistry`・制約型 |
| `mtoon` | `MToonMaterial`・テクスチャ/ユニフォーム・`OutlineWidthMode` |
| `renderer` | `VrmRenderer` |
| `camera` | `OrthographicCamera`・`ModelUniform`・ビュー空間ヘルパー |
| `viseme` | `VisemeAnalyzer`・`VisemeWeights`（リップシンク） |
| `beat_sync` | `BeatSway` |
| `debug_renderer` | `DebugRenderer`・線/球ヘルパー |
| `layer_composer` | `MotionLayer` |
| `error` | `VrmError`・`VrmResult` |

## 依存関係

- 依存: 内部なし（wgpu・gltf・glam・image など）。
- 利用: `ene-desktop` と `ene-stage`。

## リファクタリングの注目点

- **サポート対象 API と内部の区別**: `prelude` と
  [API リファレンス](../api/ene-vrm.md) がサポート対象の部分集合を示します。
  多くのサブパーサー（`load_humanoid_bones`・`load_look_at`・
  `load_mtoon_materials` など）は `#[doc(hidden)]` で、`load_vrm` からのみ
  呼ばれます。可視性を保つか、先にサポート対象を明示的に広げてください。
- 分離は契約です。ここに mind/runtime/store 依存を追加するのはアーキテクチャ
  違反です。
