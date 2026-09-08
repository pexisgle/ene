# 実装進捗

## 現在の milestone

- Stage 2 Setup とテキスト会話の最初の縦断 slice（未着手）

## 完了した milestone

- Stage 0 repository / build foundation
- Stage 1 最小 foundation と共有 contract（`ene-primitive` / `ene-config` / `ene-api` 最小 DTO / Host・CLI 最小 entrypoint / owner-local error 規約）

## 次に進む領域

- Stage 2: Host↔Client 接続・handshake、Setup、Permission / Credential / Inference 最小 boundary、単一 Provider 経路、Host 発行 round、streaming、History 保存・復元、integration test
- `ene-stage`（GUI Client）は Stage 2 の presentation 需要時点で追加（Stage 1 は CLI-first）
- shared error crate は作らない（owner-local 維持）

## 未解決 blocker

- なし
