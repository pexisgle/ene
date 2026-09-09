# 実装進捗

## 現在の milestone

- Stage 2 Setup とテキスト会話の最初の縦断 slice（レビュー指摘対応中）

## 完了した milestone

- Stage 0 repository / build foundation
- Stage 1 最小 foundation と共有 contract

## 次に進む領域

- Stage 2 rework（#1360）: production 経路の E2E が green になるまで Stage 2 完了にしない
  - 実 listener bind・実 CLI builder・generation 受け渡し・presence lifecycle 分離・pairing 承認・ingress gate・mutex 分割・single-instance・confirm 送信・consent CAS・idempotency 確定
  - 下位 restack: ConsentRepository CAS・device pairing 契約・setup 共有 grammar（#1356）、CAS 実装・local_id 列・device テーブル（#1358）
- 上記 stack の bottom-up merge 後に Stage 3（Learning）へ進む
- shared error crate は作らない（owner-local 維持）

## 未解決 blocker

- #1360 レビュー指摘の E2E green 化（対応中）
- マージ順: #1355 → #1356 → #1358 → #1359 → #1360
