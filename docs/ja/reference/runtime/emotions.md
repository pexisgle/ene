# 感情と Performance

Ene は mind ストリーム内で、**LLM Performance マーカー**（`mind.emotion.enabled` が false）と **エンジン管理の感情**（Output Arbiter → `Performance` cue）の 2 つの提示機構をサポートします。

API v1 では、チャットコンシューマーは [`EneEvent::Performance`](streaming-events.md) を受け取ります — 単独の `SpecialToken` や `Expression` イベントではありません。

## Mind ランタイムパス

`mind.emotion.enabled` が true の場合:

1. **プレターン:** `EmotionEngine` が DB から `AffectState` を読み込み、時間減衰と決定論的評価（感謝・称賛・侮辱・緊急・疲労）を適用。前ターン post-turn で保存された分類器提案があれば 1 回だけマージ。
2. **プロンプト:** `build_natural_dialogue_contract()` がトークンリスト PHI を置換。LLM には自然な会話のみを要求（Performance マーカー不要）。
3. **ストリーム後:** `OutputArbiter` が更新済み `AffectState`（+ 任意の LLM トークン）を提示 cue にマッピング。ヒステリシス適用。
4. **イベント:** `EneEvent::Performance { turn, cues, source }` をコンシューマーへ送出。
5. **ポストターン:** 更新済み `AffectState`（最終表情 / cue 状態を含む）を `Terminal` 前に永続化。

```
ユーザー入力
  → EmotionEngine（減衰 + 評価 [+ 前ターンの pending 分類器提案]）
  → PromptPacket（Current Mood + 自然対話契約）
  → LLM ストリーム（テキストのみ）
  → OutputArbiter
  → EneEvent::Performance
  → VRM / CLI 表示
```

`PerformanceCue` / `CueSource` は `ene-mind` が所有（runtime が再エクスポート）。デスクトップは cue 名を VRM 再生へマップし、`ene-vrm` に mind を依存させません。

### 設定

[settings.md](../configuration/settings.md) の `mind.emotion` を参照:

| キー | 役割 |
|-----|------|
| `enabled` | エンジン管理感情のマスタースイッチ |
| `decay_half_life_minutes` | ターン間の PAD 中立への減衰 |
| `expression_hysteresis_seconds` | 表情 / cue 変更前の最小保持時間 |
| `llm_expression_is_advisory` | true のときストリームトークンは arbiter 用に蓄積 |
| `classifier_timeout_secs` / `classifier_min_confidence` | post-turn 非同期分類器の予算とマージゲート (#88) |
| `classifier_language` | 分類器と自然対話契約のロケール（`en` / `ja`） |

分類器のモデルとプロバイダは `ai.tasks.classifier` から解決されます（[settings.md](../configuration/settings.md) 参照）。

### Post-turn 非同期分類器

`mind.emotion.enabled` が true のとき、アシスタント応答の **後** に affect 分類器を実行します。

- 入力: ターン開始時の `AffectState` スナップショット + 最近の会話履歴（現在の `user + assistant` を含む）
- 出力: `valence` / `arousal` / `irritation` / `affinity` の絶対推定
- 成功: `source_turn_id = N` の pending として保存し、次プレターンで `current_user_turn == N + 1` のとき一度だけブレンド（`confidence` で重み付け）
- 失敗/タイムアウト: ログして無視（決定論パスは継続）
- 古い / 未来の pending は破棄

**INFO** レベルで次を確認できます:
- `Starting post-turn affect classifier`
- `Post-turn affect classifier estimate complete`
- `Blended post-turn classifier estimate into affect`（次ターン開始時）

分類器ログが無い場合は `mind.emotion.enabled` が true か、`ai.tasks.classifier` が設定されているか確認してください。

## Performance マーカーパス

感情エンジン無効時、LLM は `<|perf:…|>` マーカーを出力できます。ストリームタスクは `TextDelta` からマーカーを除去し、Performance パスがそれらを `Performance` cue として表面化します。

### マーカー解析

mind の special-token ヘルパー:

| 関数 | 説明 |
|----------|-------------|
| `split_text_and_special_tokens(carry, chunk)` | チャンクをテキストと `<\|...\|>` に分割。境界跨ぎは `carry` |
| `parse_performance_marker(token)` | `<\|perf:…\|>` を `PerformanceCue` に解析 |

### データフロー

```
LLM ストリーム → 生テキスト
  ↓
ene-runtime / mind ストリームパス
  ├── テキスト → EneEvent::TextDelta { turn, delta }
  └── <|perf:…|> → テキストから除去。Performance cue になる
       ↓
コンシューマー:
  ├── CLI: TextDelta → 表示; Performance → cue 名ログ
  └── Desktop: TextDelta → AI テキスト; Performance → PerformanceCue / EmoteToken → VRM
```

### Performance Output Protocol（PHI）

感情無効時、`build_expression_phi()` が `card.data.extensions["expressions"]` 由来の `<|perf:expr=NAME|>` マーカーと `<|perf:…|>` 文法を列挙する指示ブロックを注入します。

## アプリ別処理

| アプリ | チャットイベント | 下流 |
|-------------|------------|------------|
| ene-desktop | `Performance` | `AppEvent::PerformanceCue` → `EmoteToken` → VRM |
| ene-cli | `Performance` | cue 名の表示 / ログ |

デスクトップの保持時間は `mind.emotion.expression_hysteresis_seconds`（既定 4.0s）に従います。

## 関連ドキュメント

- [アバター Performance ADR](../architecture/avatar-performance.md) — `PerformanceCue` マーカー、調停、VRM LayerComposer
- [ストリーミングイベント](streaming-events.md) — `EneEvent::Performance`
