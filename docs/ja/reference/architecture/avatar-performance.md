# アバター Performance システム

- **Status:** Accepted
- **Date:** 2026-07-14

## 概要

Performance システムは、表情、モーション、視線制御を単一の `PerformanceCue` 型の下に統合します。Cue は3つのソース（LLM ストリームマーカー、アフェクトエンジン、キャラクターヒステリシス）から発生し、VRM レンダラーに到達する前に優先度によって調停されます。

## アーキテクチャ

```
テキストストリーム ──▶ parse_performance_marker() ──▶ PerformanceArbiter
                      │                              │
アフェクトエンジン ───┼──────────────────────────────┤
                      │                              │
キャラクター設定 ────┼──────────────────────────────┤
  (モーションカタログ,  │                              ▼
   表情)              │                     resolve() → Vec<(PerformanceCue, CueSource)>
                      │                              │
                      ▼                              ▼
                LayerComposer  ◀────────  EneEvent::Performance
                ExpressionCompositor
                      │
                      ▼
                VRM レンダラー (ブレンドシェイプ, ボーンアニメーション, look-at)
```

## データフロー

### 1. ストリームマーカー解析

LLM はストリーミングテキスト中に `<|perf:…|>` トークンをインラインで出力できます。`<|emo:NAME|>` は `<|perf:expr=NAME|>` の表情省略形です。

| マーカー | 例 | 効果 |
|--------|---------|--------|
| Expression（省略形） | `<\|emo:happy\|>` | 表情ブレンドシェイプを設定 |
| Expression | `<\|perf:expr=happy\|>` | 表情ブレンドシェイプを設定 |
| Expression（重み付き） | `<\|perf:expr=happy,weight=0.8,hold=2.0\|>` | 重みと保持時間付き |
| Motion | `<\|perf:motion=wave,layer=upper\|>` | 上半身モーションを再生 |
| Look-at | `<\|perf:lookat=user\|>` | 対象への視線移動 |
| Cancel | `<\|perf:cancel=expr\|>` / `<\|perf:cancel=motion\|>` / `<\|perf:cancel=all\|>` | スロットをクリア |

`ene_mind::session::special_token::parse_performance_marker()` と `extract_emotion_from_token()` を参照。

### 2. Performance Arbiter

`PerformanceArbiter` はターン中に Cue を収集し、ターン終了時に最終セットを解決します。

**優先度順（高い方が優先）:**
| 優先度 | ソース | 値 |
|----------|--------|-------|
| 5 | `LlmCommand` | LLM トークン、`llm_expression_is_advisory = false` |
| 4 | `LlmAdvisory` | LLM トークン、アドバイザリーモード on |
| 3 | `Affect` | アフェクト→表情マッピング |
| 2 | `Hysteresis` | 前回の表情を保持 |
| 1 | `Fallback` | ニュートラルまたは最近傍 |

- 同一優先度: 最新が優先
- 異なる Cue 種別（expression, motion, look-at）は独立したスロット
- Cancel Cue は該当スロットをクリア

`ene_mind::output::performance_arbiter::PerformanceArbiter` を参照。

### 3. モーションカタログ

キャラクターカードは `extensions.ene` で利用可能なモーションを宣言します:

```json
{
  "extensions": {
    "ene": {
      "motion_catalog": {
        "idle_lower": "idle_basic",
        "motions": [
          { "name": "wave",   "path": "motions/wave.vrma",   "layer": "upper" },
          { "name": "idle_basic", "path": "motions/idle.vrma", "layer": "lower" }
        ]
      }
    }
  }
}
```

- `MotionLayer::Upper` — 上半身ジェスチャー（腕、頭）。Lower と共存
- `MotionLayer::Lower` — 下半身アイドルループ。Upper と共存
- `MotionLayer::Full` — 全身オーバーライド。Upper と Lower をプリエンプト
- `idle_lower` — デスクトップレンダリング用のデフォルトアイドルループ名

### 4. Layer Composer

`LayerComposer` は3つのモーションレイヤーを管理します:

```
Full ──▶ Upper + Lower をプリエンプト
Upper ──▶ Lower と共存（上半身ジェスチャー）
Lower ──▶ Upper と共存（アイドルループ）
```

`ene_vrm::layer_composer::LayerComposer` を参照。

### 5. Expression Compositor

`ExpressionCompositor` はカード定義の表情ウェイトとランタイムオーバーライドをマージします:

1. **ベースレイヤー** — カードの表情ブレンドシェイプマップ（例: `"happy" → {"happy": 0.8, "mouthSmile": 0.5}`）
2. **オーバーライドレイヤー** — `PerformanceCue` からのブレンドシェイプ単位のランタイムオーバーライド
3. **合成** — ベース ∪ オーバーライド。一致するキーはオーバーライドが優先

`ene_vrm::expression_compositor::ExpressionCompositor` を参照。

### 6. デスクトップ結線

- `EneEvent::Performance` → `ai_bridge.rs` が `AppEvent::PerformanceCue`（表情）または `AppEvent::MotionCue`（モーション）にルーティング
- Expression パス: `EmotionPipelineState`（保持/フェード） → VRM `ExpressionsLayer`
- Motion パス: `MotionLayerState`（`LayerComposer` をラップ） → VRM `VrmaPlayer`
- `cue_source_to_u8()` が `CueSource` を整数優先度にマッピング

## Prompt 契約

感情エンジンが**無効**の場合、LLM は post-history PHI ブロックで Performance 文法指示を受け取ります（旧来の `<|emo:NAME|>` 契約を置換）:

```
## Performance Output Rule
RULE: You may use special tokens to control your avatar's expression, motion, and gaze.
Place each token BEFORE the sentence it describes.

Grammar:
  Expression (required, shorthand): `<|emo:NAME|>`
  Expression (full): `<|perf:expr=NAME[,weight=0.0-1.0][,hold=SECS]|>`
  Motion: `<|perf:motion=NAME[,layer=upper|lower|full]|>`
  Look-at: `<|perf:lookat=TARGET|>`
  Cancel: `<|perf:cancel=expr|motion|all|>`

Available expressions:
- `<|perf:expr=happy|>` — Feeling joyful, excited, or pleased
...
```

感情エンジンが**有効**の場合、自然対話契約は LLM にプレーンテキストのみを使用するよう指示します — ランタイムがアフェクト状態から表情を管理します。

## クレート所有権

| 型 | クレート | 備考 |
|------|-------|-------|
| `PerformanceCue`, `CueSource`, `PerfKind`, `MotionLayer` (cue) | `ene-mind` | `ene-runtime` が再エクスポート |
| `PerformanceArbiter` | `ene-mind` | ターン中調停 |
| `MotionLayer` (config), `MotionCatalog`, `MotionEntry` | `ene-config` | キャラクターカード上のシリアライズ |
| `LayerComposer`, `MotionLayer` (vrm) | `ene-vrm` | mind/runtime から独立 |
| `ExpressionCompositor` | `ene-vrm` | 表情ウェイトマージ |

## 関連ドキュメント

- [感情と Performance](../runtime/emotions.md) — アフェクトエンジンと表情マッピング
- [ストリーミングイベント](../runtime/streaming-events.md) — チャットイベントバス
- [API v1 ADR](api-v1.md) — ホスト契約とクレートマップ
