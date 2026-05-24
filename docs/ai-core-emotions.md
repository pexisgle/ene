# 感情表現トークン

LLM が生成する `<|emo:name|>` 形式の特殊トークンにより、キャラクターの表情を制御する。

## トークン解析

`special_token.rs` で実装:

| 関数 | 説明 |
|------|------|
| `split_text_and_special_tokens(carry, chunk)` | ストリームチャンクをテキスト断片と `<|...|>` トークンに分割。トークンがチャンクを跨ぐ場合は `carry` で保持 |
| `extract_emotion_from_token(token)` | `<|emo:happy|>` から感情名 `"happy"` を抽出（大文字小文字不区別） |

## データフロー

```
run_ai_with_tools → TextDelta(String)
  ↓
コンシューマー（ai_bridge / CLI) が受信
  ↓
session.process_delta(chunk) で分割
  ├── テキスト → 表示
  └── <|emo:name|> → 感情トークンとして処理
       ↓
GUI: EmotionQueue → VRM SetExpressions で表情適用
CLI: "[Emotion: name]" をマゼンタ表示
```

## Emotion Expression Protocol

`build_expression_phi()` がプロンプトに注入する指示ブロック。キャラクターカードの `extensions["expressions"]` から利用可能な感情一覧を生成する。

デフォルト表情（カード側で無効化可能）:

| 感情 | VRM Blendshape マッピング |
|------|--------------------------|
| neutral | デフォルト |
| happy | 定義値 |
| sad | 定義値 |
| angry | 定義値 |
| relaxed | 定義値 |
| surprised | 定義値 |

`post_history_instructions` とマージされ、LLM に感情トークンの使用を指示する。

## アプリケーションごとの処理

| アプリ | 処理 |
|--------|------|
| ene-desktop（GUI） | `TextDelta` を `process_delta()` で解析 → `EmotionQueue` → `process_emotion_queue`（4秒ホールド→フェードアウト） → `SetExpressions` で反映 |
| ene-cli（CLI） | `TextDelta` 内のトークンを `process_delta()` で解析、`[Emotion: name]` としてマゼンタ表示 |

※ `AiStreamEvent::SpecialToken` は現在の実装では使用されない。
