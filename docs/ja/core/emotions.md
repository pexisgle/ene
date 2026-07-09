# 感情・表情

Ene は **レガシートークンモード**（`cognition.emotion.enabled` が false、またはレガシーストリーミングパイプライン）と **認知ランタイムモード**（エンジン管理の感情 + Output Arbiter）の 2 つの表情パスをサポートします。

## 認知ランタイムパス（`cognition.enabled` 時のデフォルト）

`cognition.emotion.enabled` が true の場合:

1. **プレターン:** `EmotionEngine` が DB から `AffectState` を読み込み、時間減衰と決定論的評価（感謝・称賛・侮辱・緊急・疲労）を適用。前ターン post-turn で保存された分類器提案があれば 1 回だけマージ。
2. **プロンプト:** `build_natural_dialogue_contract()` がトークンリスト PHI を置換。LLM には自然な会話のみを要求（`<|emo:|>` 不要）。
3. **ストリーム後:** `OutputArbiter` が更新済み `AffectState`（+ 任意の LLM トークン）を表情にマッピング。ヒステリシス適用。
4. **イベント:** `EneEvent::Expression { name, source }` をコンシューマーへ送出。
5. **ポストターン:** 更新済み `AffectState`（`last_expression` 含む）を永続化。

```
ユーザー入力
  → EmotionEngine（減衰 + 評価 [+ 前ターンの pending 分類器提案]）
  → PromptPacket（Current Mood + 自然対話契約）
  → LLM ストリーム（テキストのみ）
  → OutputArbiter
  → EneEvent::Expression
  → VRM / CLI 表示
```

### 設定

`cognition.emotion` は [settings.md](../configuration/settings.md) を参照。

| キー | 役割 |
|-----|------|
| `enabled` | エンジン管理感情のマスタースイッチ |
| `engine` | `deterministic` / `llm` / `hybrid`（分類器）— [エンジンモード](#エンジンモード) を参照 |
| `decay_half_life_minutes` | ターン間の PAD 減衰 |
| `expression_hysteresis_seconds` | 表情変更の最小保持時間 |
| `llm_expression_is_advisory` | true 時、ストリームトークンは即時送出せず Arbiter 用に蓄積 |
| `classifier_timeout_secs` / `classifier_min_confidence` | post-turn 非同期分類器の実行予算と適用ゲート（#88） |
| `classifier_language` | 分類器と自然対話契約のプロンプト言語（`en` / `ja`） |
| `classifier_model` | 分類器専用のチャットモデル（既定 `google/gemini-2.5-flash-lite`） |
| `classifier_max_tokens` | 分類器の最大 completion トークン数（`0` = 上限なし） |

### post-turn 非同期分類器

`engine` が `llm` または `hybrid` のとき、感情分類器は **応答生成後** に実行されます。

- 入力: ターン開始時の `AffectState` スナップショット + 直近の会話履歴（当該ターンの `user + assistant` を含む）
- 出力: 会話後の `valence` / `arousal` / `irritation` / `affinity` の絶対推定値
- 成功時: `source_turn_id = N`（完了した user ターン）として pending 保存し、次の pre-turn で `current_user_turn == N + 1` のとき `confidence` 重みで 1 回だけブレンド適用
- 失敗/タイムアウト時: ログのみ残して無視（決定論ルートは継続）
- 古すぎる / 未来向けの pending は破棄

**INFO** ログでは次が出ます:
- 非同期ジョブ開始時: `Starting post-turn affect classifier`
- 分類成功時: `Post-turn affect classifier estimate complete`（推定値一式）
- **次ターン** pre-turn で pending をマージしたとき: `Blended post-turn classifier estimate into affect`

分類器ログが一切出ない場合は、`cognition.emotion.engine` が `hybrid` または `llm` になっているか確認してください（`deterministic` では分類器は動きません）。

### エンジンモード

| モード | pre-turn ルール | post-turn 分類器 |
|--------|-----------------|------------------|
| `deterministic` | あり（感謝・侮辱・減衰など） | **なし** |
| `hybrid`（既定） | あり | あり — 推定は次ターンでブレンド |
| `llm` | **なし**（減衰のみ） | あり — 推定は次ターンでブレンド |

ルールベースか分類器のどちらかを明示的に無効にしたい場合以外は `hybrid` を使ってください。

分類器はメインストリームとは **別の provider インスタンス** を使い、strict JSON Schema（`response_format` + `strict: true`）、任意の `classifier_max_tokens`、OpenRouter 向けの transport フォールバックを行います。

これにより、即時の応答表現は応答LLMが担当しつつ、分類器は内部感情状態の安定化シグナルとして後段で利用されます。

## レガシートークンパス

感情エンジン無効時またはレガシーパイプラインでは、LLM が `<|emo:name|>` 特殊トークンで表情を制御できます。

### トークン解析

`special_token.rs` で実装:

| 関数 | 説明 |
|------|------|
| `split_text_and_special_tokens(carry, chunk)` | ストリームチャンクをテキスト断片と `<\|...\|>` トークンに分割。チャンクを跨ぐトークンは `carry` で保持 |
| `extract_emotion_from_token(token)` | `<\|emo:name\|>` から感情名を抽出 (大文字小文字不区別) |

### レガシーデータフロー

```
LLM ストリーム → 生テキストチャンク
  ↓
ene-core ストリームタスク: session.process_delta(chunk)
  ├── テキスト → EneEvent::TextDelta { delta }
  └── <|emo:name|> → EneEvent::SpecialToken { token }
       ↓
コンシューマーが個別のイベントを受信:
  ├── CLI: TextDelta → 直接表示
  │       SpecialToken → extract_emotion_from_token → "[Emotion: name]"
  └── デスクトップ: TextDelta → EneStreamEvent::TextDelta
              SpecialToken → extract_emotion_from_token → EmoteToken
                → EmotionQueue → 保持 → フェードアウト → SetExpressions (VRM)
```

**重要:** `TextDelta` からの感情抽出は `ene-core` のストリームタスク内で行われます。

### 感情出力プロトコル（PHI）

`build_expression_phi()` が利用可能な `<|emo:name|>` トークン一覧をプロンプトに注入。トークンは `card.data.extensions["expressions"]` から導出。

デフォルト表情（カードごとに無効化可能）:

| 感情 | VRM ブレンドシェイプ |
|------|---------------------|
| neutral | デフォルトポーズ |
| happy | 定義値 |
| sad | 定義値 |
| angry | 定義値 |
| relaxed | 定義値 |
| surprised | 定義値 |

`post_history_instructions` とマージして注入。

## アプリ別処理

| アプリ | 認知パス | レガシーパス |
|--------|----------|--------------|
| ene-desktop | `Expression` → `EmoteToken` → `EmotionPipelineState` | `SpecialToken` → `EmoteToken` |
| ene-cli | `Expression` → `[Expression: name]` | `SpecialToken` → `[Emotion: name]` |

デスクトップの保持時間は `cognition.emotion.expression_hysteresis_seconds`（デフォルト 4.0 秒）に従います。
