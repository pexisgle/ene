# セッション分割と圧縮

**hard-split は製品経路ではない。** 文脈境界は mind ランタイムの rolling compression（`mind.context.compression_*`）を使う。分割スコアによる session-ID 発行はレガシー／明示時のみ。

## 現在のポリシー

- **Compression（製品経路、`mind.context.compression_enabled=true`）**
  - compression が権威のとき自動 hard-split はバイパス
  - 古いターンは `memory_spans` に圧縮
  - `session_id` は維持される
- **Hard-split（非推奨、`session.auto_split` デフォルト `false`）**
  - コンパニオン UX では推奨しない
  - 明示的に有効かつ compression オフのとき、複合スコアで分割し新しい `session_id` を発行しうる
  - 手動 `/session split` は運用／デバッグ用に残る場合がある；cognition+compression 時は手動 compression 挙動を優先

## 圧縮が優先される理由

- コンパニオン体験の継続性を保つ
- 人工的な会話断絶を減らす
- 長文脈でもプロンプト予算を安定化できる

## レガシー分割の理由

- Timeout
- TopicChange
- Manual

## 運用メモ

- pending な split/compression タスクは同時に 1 つのみ処理されます。
- mind + compression モードでの手動 split は、実体として手動 compression 挙動にルーティングされます。
- legacy hard-split が発生した場合は apply 後に新しい `session_id` が発行されます。

## 関連ドキュメント

- `docs/ja/architecture/api-v2.md`
- `docs/ja/architecture/cognitive-runtime.md`
- `docs/ja/core/session.md`
- `docs/ja/configuration/settings.md`（`session.auto_split`、`mind.context.compression_*`）
