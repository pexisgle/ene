# ene-vrm API

`ene-vrm` は `wgpu` 29 と `gltf` 1.4 上に構築されたプラットフォーム非依存の
VRM 1.0 ローダー兼レンダラーです。`ene-desktop` が使用します。このページは
**意図的にサポートされる API** を挙げます（他の `pub` 項目は呼び出し可能
ですがサポート対象外です）。

## 最初に: `prelude`

`ene_vrm::prelude` が厳選されたサポート対象 API を再エクスポートします。
特に断りのない限り下記はすべて含まれます。

## 読み込み

```rust
let model = ene_vrm::load_vrm(path)?;            // VrmModel
let clip = ene_vrm::load_vrma(path)?;            // VrmaClip（アニメーション）
```

| 項目 | 目的 |
|---|---|
| `loader::load_vrm` | `.vrm` ファイルを `VrmModel` にパース（メッシュ・スケルトン・表情・spring bone・視線・マテリアル） |
| `model::VrmModel` / `VrmMesh` / `VrmTexture` | 読み込まれたモデルデータ |
| `error::VrmError` / `VrmResult` | 統合エラー型 |

## アニメーション

| 項目 | 目的 |
|---|---|
| `VrmaClip` / `VrmaFrame` | パース済み VRMA クリップ: ボーン/表情/視線チャネルとキーフレーム補間 |
| `VrmaPlayer` | 再生: シーク・再生・繰り返しモード・ブレンドウェイト |
| `evaluate_clip` | クリップを時刻でサンプリングしボーン/表情出力へ |
| `retarget_rotation` / `retarget_hips_translation` | リターゲットヘルパー |
| `Interpolation` / `RepeatMode` | チャネル補間とループ |
| `MotionLayer`（`layer_composer` 内） | 重なり合うモーションのレイヤー合成 |
| `BeatSway`（`beat_sync`） | 音楽同期のビート駆動スウェイ |

## 表情

| 項目 | 目的 |
|---|---|
| `ExpressionLayer` / `ExpressionName` | ブレンドシェイプレイヤーと標準名 |
| `expression_compositor` | 複数の表情レイヤーを合成 |
| `expression_override` | 手続き的オーバーライド: まばたき・視線・口のターゲット（`apply_overrides`） |
| `viseme` | 音声駆動の口形分析（リップシンクウェイト） |

## スケルトンと視線

| 項目 | 目的 |
|---|---|
| `HumanoidBoneRegistry` / `VrmBone` | 規格の 55 ボーン → glTF ノード。`canonicalize_bone_name` |
| `LookAtEvaluator` / `LookAtProperties` | レンジマップ付きの毎フレーム視線評価 |
| `NodeConstraintRegistry` | ノード制約（aim/roll/rotation） |
| `SpringBone` | spring bone シミュレーション（髪/服） |

## レンダリング

| 項目 | 目的 |
|---|---|
| `renderer` | wgpu レンダーパイプライン・バインドグループ・テクスチャ管理 |
| `camera::OrthographicCamera` | カメラユニフォーム + ビュー射影ヘルパー（`ndc_to_view_pos`・`pixel_to_ndc`・`view_pos_to_world`） |
| `mtoon::MToonMaterial` | MToon マテリアル対応（テクスチャ・ユニフォーム・アウトライン） |
| `post_process` | 後処理パイプライン |
| `debug_renderer` | デバッグ線/球の描画ヘルパー |

## 例

- `examples/diagnostic_model_matrix.rs` — モデル読み込み診断。
- `examples/inspect_aabb.rs` — バウンディングボックス検査。

## スコープ

- VRM **1.0** のみ（`.vrm`・`.vrma`）。
- 描画のみ。認知・メモリ・ランタイム型はインポートされません。
- 一部のローダー（`load_humanoid_bones`・`load_look_at`・
  `load_spring_bones`・`load_mtoon_materials` など）は `load_vrm` からのみ
  呼ばれるため `#[doc(hidden)]` です。ただし型はモデルの public フィールドの
  一部として `pub` のままです。
