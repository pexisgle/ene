# ターンとストリーミング

**ターン**は、ユーザーメッセージの受信から、LLM によるストリーミングと必要に応じたツール呼び出しを経て、ランタイムが Terminal チャットイベントを出すまでの単位です。

## ホスト API（概念モデル）

1. `EneHandle::open(config, card)` — 準備済みハンドル。プロバイダ・store・ツール・mind が揃ってから返る
2. `run(input)` — ターンを開始し `TurnId` を返す。別ターン実行中なら `Busy`
3. `subscribe()` — そのターンのストリーミングチャットイベントを受信
4. `cancel(turn)` — 一致するターンだけキャンセル

同時に走るターンは 1 つだけです（single-flight Busy）。

## 見えるイベント

典型的には: ストリーム差分 → 権限 / ユーザー入力の要求 → ツール結果 → アバター用 **Performance** → **Terminal**。

診断（スナップショット、ツール一覧、手動セッション分割）は `diagnostics()` 経由で、チャットバスではありません。

## さらに深く

- [ストリーミングエンジン](../../reference/runtime/streaming.md)
- [ストリーミングイベント](../../reference/runtime/streaming-events.md)
- [API v1 ADR](../../reference/architecture/api-v1.md)
