# 実装進捗

## 現在の milestone

- M2 テキスト会話の縦断 slice（未着手）

## 完了した milestone

- M0 旧実装の退避確認と workspace の再構成
- M1 基盤 crate の新規構築（`ene-primitive` / `ene-config` / `ene-error`）

## 次に進む領域

- M2: `ene-store` 基盤（app.db、Conversation History、最小 presence）、round 発行 authority（`ene-presentation`）、IPC V-1/V-2、`ene-api` wire-neutral DTO
- M2 以降: 対象 crate 登場時に CI へ feature-matrix / windows-native を追加（M1 では対象 crate なしのため最小 CI のみ）

## 未解決 blocker

- なし
