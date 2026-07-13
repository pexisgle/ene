# セッション分割と圧縮

現在の推奨パスは「自動分割」ではなく、mind ランタイムの rolling compression です。

## 現在のポリシー

- **Mind compression モード（`compression_enabled=true`）**
  - 自動セッション分割はバイパス
  - 古いターンは `memory_spans` に圧縮
  - `session_id` は維持される
- **ハード分割モード（`compression_enabled=false`）**
  - 複合スコアによる自動分割が有効
  - 手動 `/session split` も利用可能

## 圧縮が優先される理由

- コンパニオン体験の継続性を保つ
- 人工的な会話断絶を減らす
- 長文脈でもプロンプト予算を安定化できる

## ハード分割の理由

- Timeout
- TopicChange
- Manual

## 運用メモ

- pending な split/compression タスクは同時に 1 つのみ処理されます。
- mind + compression モードでの手動 split は、実体として手動 compression 挙動にルーティングされます。
- legacy split が発生した場合は apply 後に新しい `session_id` が発行されます。

## 関連ドキュメント

- `docs/ja/architecture/cognitive-runtime.md`
- `docs/ja/core/session.md`
