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
              SpecialToken → extract_emotion_from_token → EneStreamEvent::SpecialToken
                → EmotionQueue → SetExpressions (VRM ブレンドシェイプ)
```

**重要:** `TextDelta` からの感情抽出は `ene-core` のストリームタスク内で行われ、コンシューマー内ではありません。コンシューマーは事前にパースされた `SpecialToken` イベントを受信します。`TextDelta` に対して `extract_emotion_from_token` を呼び出す必要はありません。

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
| ene-desktop (GUI) | `SpecialToken` → `extract_emotion_from_token` → `EneStreamEvent::SpecialToken` → `EmotionQueue` → 4秒ホールド → フェードアウト → `SetExpressions` |
| ene-cli (CLI) | `SpecialToken` → `extract_emotion_from_token` → `[Emotion: name]` マゼンタ表示 |
