# 実装進捗

## 現在の milestone

- Stage 2 Setup とテキスト会話の最初の縦断 slice — merge 準備完了（production blocker 0）

## 完了した milestone

- Stage 0 repository / build foundation
- Stage 1 最小 foundation と共有 contract

## Stage 2 で成立したもの

- Production 経路: `ene-core serve`（Unix socket listener、same-UID peer check、pairing → capability negotiation → challenge/proof 認証、Host-local device / credential approval）と実 `ene-ctl`（setup / send / watch / history）が実 binary E2E で接続。
- 会話縦断: Host 発行 round → Owner text intake → OpenAI Responses inference → stream / presentation confirm → durable history。
- 再起動復元: 保存済み timeline の復元と、Host 再起動後の継続送信（実 binary E2E `binaries_drive_send_stream_history_and_restart`）。
- Command replay: `RequestFingerprint` = role + body + language + sending incarnation + canonical client round intent が唯一の replay 比較。accepted result（domain round / Host 発行 wire projection）は replay 判定に使わない。same id + different content は `ConflictingCommand`、restart 後も exact retry は stored accept を verbatim replay。
- Round: `RoundIntentMark::{Auto, New, Existing}` が canonical client intent。force-new は round premise を持たず（premise があれば自己矛盾として `StaleRound`）、round projection は同一 round への並行 submit でも atomic get-or-create で単一 wire。
- ManagementIntent: `intent_id` keyed durable replay を write-once で保持（plain INSERT、in-transaction claim check、race loser は winner を再読）。malformed consent target も `record_decided()` を通り、store failure 時は decided outcome を返さず `HeldByOperation`（durable-before-visible）。
- Inference attempt start の linearization、consent / currentness の compare-before-commit、transport duplicate suppression。
- Validation: workspace unit / store / vertical slice と production binary E2E。Linux / Windows CI green。

## 未解決 blocker

- なし（Stage 2 production blocker: 0）

## Deferred（later-stage tracking）

- Stage 5 Client lifecycle / presence / recovery（[実装ガイド](README.md) の Stage 5 で扱う。#1360 review threads）:
  - current authenticated connection 消失後の presence fallback edge。
  - superseded connection が同一 socket 上で handshake phase へ戻れる問題。
- P2 / later hardening（#1360 review threads）:
  - Client lost-reply retry API: 最初の `request()` が生成した command id を caller が保持できない。
  - `ClientIncarnationId` の process-boot semantics。
  - paired connection の missing `device_id` strictness と post-auth `Reject` sender。
  - display descriptor と multi-device pairing identity の分離。
  - future / unknown `WirePayload` variant の codec compatibility。

## merge 順

- #1355 → #1356 → #1358 → #1359 → #1360（bottom-up）

## 次の Stage

- Stage 3 Experience Summary / Memory。共有 contract が安定していれば Stage 4 Task / Action を並列で開始可能（[実装ガイド](README.md) の並列化条件に従う）。
