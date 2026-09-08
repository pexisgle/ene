# 実装進捗

## 現在の milestone

- Stage 3 Experience Summary / Memory（未着手）

## 完了した milestone

- Stage 0 repository / build foundation
- Stage 1 最小 foundation と共有 contract
- Stage 2 Setup とテキスト会話の最初の縦断 slice（Host↔Client 接続・Setup・最小 boundary・単一 Provider 経路・Host 発行 round・streaming・History 保存・再起動復元・integration test）

## 次に進む領域

- Stage 3: Stage 2 の Conversation History を根拠に Learning 追加（`ene-learning`、Summary/Memory形成、通常忘却と削除の区別）
- `ene-stage`（GUI Client）は presentation 需要時点（Stage 2 の CLI-first を継続）
- 先送り維持: fallback/複数 Provider・Voice・Observation・Task・Remote・backup/Restore（各 Stage で）
- shared error crate は作らない（owner-local 維持）

## 未解決 blocker

- なし
