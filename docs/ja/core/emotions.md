# 感情トークン

LLM は `<|emo:name|>` 形式の特殊トークンを生成して、キャラクターの表情を制御できます。

## トークン解析

`special_token.rs` で実装:

| 関数 | 説明 |
|------|------|
| `split_text_and_special_tokens(carry, chunk)` | ストリームチャンクをテキスト断片と `<\|...\|>` トークンに分割。チャンクを跨ぐトークンは `carry` で保持 |
| `extract_emotion_from_token(token)` | `<\|emo:name\|>` から感情名を抽出 (大文字小文字不区別) |

## データフロー

```
run_ai_with_tools → TextDelta(String)
  ↓
コンシューマー (ai_bridge / CLI) が受信
  ↓
session.process_delta(chunk) で分割
  ├── テキスト → 表示
  └── <|emo:name|> → 感情トークン処理
       ↓
GUI: EmotionQueue → SetExpressions (VRM ブレンドシェイプ)
CLI: "[Emotion: name]" マゼンタ表示
```

## 感情表現プロトコル

`build_expression_phi()` が利用可能な `<|emo:name|>` トークンの一覧をプロンプトに注入します。トークンは `card.data.extensions["expressions"]` から導出されます。

デフォルトの表情 (カードごとに無効化可能):

| 感情 | VRM ブレンドシェイプ |
|------|-------------------|
| neutral | デフォルトポーズ |
| happy | 定義値 |
| sad | 定義値 |
| angry | 定義値 |
| relaxed | 定義値 |
| surprised | 定義値 |

`post_history_instructions` とマージされて注入されます。

## アプリケーションごとの処理

| アプリケーション | 処理 |
|----------------|------|
| ene-desktop (GUI) | `TextDelta` → `process_delta()` → `EmotionQueue` → `process_emotion_queue` (4秒ホールド → フェードアウト) → `SetExpressions` |
| ene-cli (CLI) | `TextDelta` → `process_delta()` → `[Emotion: name]` マゼンタ表示 |
