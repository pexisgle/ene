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

- #1360 レビュー第5ラウンド対応済み: replay-before-attach 並べ替え（回帰テスト付き）、assign intent replay（`IntentReplayRepository`＋V6＋conflict は clarify、register/complete は収束性により対象外と明記）、Windows stub 修正。残りは返信済み：pre-send TOCTOU は具体策の指定待ち（lock-across-IO なしで観測可能な改善なしと分析）、round_view fingerprint は durable 列がない旨説明、transport retry API は P2 downgrade 受諾
- CI 再実行待ち（Unix green、Windows は stub 修正で解消見込み）
- マージ順: #1355 → #1356 → #1358 → #1359 → #1360
