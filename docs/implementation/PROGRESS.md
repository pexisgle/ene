# 実装進捗

## 現在の milestone

- Stage 2 Setup とテキスト会話の最初の縦断 slice（レビュー指摘対応中）

## 完了した milestone

- Stage 0 repository / build foundation
- Stage 1 最小 foundation と共有 contract

## 次に進む領域

- Stage 2 rework（#1360）: production 経路の E2E は green（実 binary E2E `binaries_drive_send_stream_history_and_restart` 追加）。レビュー残件の第2ラウンド対応済み、レビュアー応答待ち
  - 追加対応: replay fingerprint（round/role/text/lang/round-wire/incarnation）と `CommandConflict` → wire reject、durable replay ack（restart-safe）、opaque device/round/presence 投影、`find_device_by_wire` による proof 解決、negotiated-major 強制、management shortcut の base 前提強制、credential approval 単一 TX、client retry API（`retry`/`retry_frame`）
  - 下位 restack: opaque wire 契約・`CommandConflict`・`RejectKind::ConflictingCommand`（#1356）、V5 migration・fingerprint 比較・atomic approval（#1358）
- 上記 stack の bottom-up merge 後に Stage 3（Learning）へ進む
- shared error crate は作らない（owner-local 維持）

## 未解決 blocker

- #1360 レビュー第10ラウンド（レビュー 5158798688＋コメント 5613441143）対応済み・CI success 確認済み: request fingerprint/accepted result 分離、round 1:1 projection、client テスト Result 化、roundtrip/decide_frame 一本化、payload_kind→message_type 委譲
- #1360 レビュー第11ラウンド対応: replay の semantic identity を一箇所へ収束。canonical client round intent（Auto / force-new / explicit join）を request fingerprint に追加し、restart 対比は schema V8（`round_intent` 列）で durable 化、pre-mark row は fail-closed（`CommandConflict`）。Host 側の先行判定は store と同一の `RequestFingerprint` 比較に一本化（`command_matches` 削除、dead `RevalidationReason::CommandMismatch` 削除）。round projection は同一 round への並行 submit でも単一 wire を返す atomic get-or-create。`retry_after_round_advance_replays_the_stored_accept` を exact retry（request fields 不変）に修正し、fresh/round-intent 変更 conflict・同 round 並行 join・restart verbatim replay の回帰テスト追加、round regression tests の silent pass を Result 化で解消。層別コミット済み（contracts / store / wiring）、CI success 確認済み
- #1360 レビュー第12ラウンド対応: force-new の carrier rule を一つに統一。`fresh=true` は設計の round-less 新規 round 要求（§13.1: `round=None`, `round_view=None`）で premise を持たないため、payload `round` / envelope `round_view` のいずれかに premise があれば自己矛盾として `StaleRound` で decline（黙って flag 扱いにしない）。DTO rustdoc（#1356）を「ignored」から carrier rule へ更新し、`fresh + round` / `fresh + round_view` / `fresh + mismatched` / legal premise-free force-new の回帰テスト追加。層別コミット済み、CI success 確認済み
- 残りは返信済み：TOCTOU・intent・replay スレッドは実装で応答、transport retry P2 受諾、Stage 5 defer 群は継続
- マージ順: #1355 → #1356 → #1358 → #1359 → #1360
