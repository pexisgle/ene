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

- #1360 レビュー第8ラウンド対応済み＋残り1分岐（malformed assign target の Clarify 未記録）も修正：`record_decided` 経由に統一し回帰テスト追加。層別コミット済み、CI success 確認済み
- #1360 レビュー第10ラウンド（レビュー 5158798688＋コメント 5613441143）対応済み：request fingerprint/accepted result 分離（store で round/wire を conflict 判定から除外、in-tx 一本化）、round 1:1 projection（既知 round は wire 再利用）、client テスト Result 化（silent pass 撲滅）、roundtrip/decide_frame 一本化、payload_kind→message_type 委譲、旧 rustdoc 一掃。層別コミット済み、CI 待ち
- 残りは返信済み：TOCTOU・intent・replay スレッドは実装で応答、transport retry P2 受諾、Stage 5 defer 群は継続
- マージ順: #1355 → #1356 → #1358 → #1359 → #1360
