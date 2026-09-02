# Harness v1.0 Tracker

> done.md の各 v1.0 項目を実プロセス検証へ紐付ける。機構検出と完成宣言を分離する。

## 方針
- done.md の [ ] には実装 issue または対象外理由+後継 milestone を対応付ける
- test 名の存在だけで check しない
- 手動 GUI は環境/手順/観測結果を記録
- v1.0 対象外は理由と後継を文書化

## 対応表（done.md → issue）

| done.md 項目 | 状態 | 対応 |
|---|---|---|
| 総括 2: 1体が全受入条件 | 未達 | #1187 blocking で分解 |
| 総括 3: offline GGUF | 未達 | #1187 offline GGUF |
| P-102 barge-in | 未達 | #1187 barge-in |
| P-103 self-voice | 未達 | #1187 self-voice |
| P-107 2体同時 | 部分 | #1187 2体同時 |
| P-403/404 VRM lip-sync | 未達 | #1187 VRM |
| P-504 対話並行 | 部分 | #1187 job cancel 等 |
| P-506 compaction LLM | 未達 | #1187 compaction |
| P-512 ask-user | 部分 | #1187 job Q&A |
| P-608 bookmark workflow | 未達 | #1187 しおり |
| P-610 skill | 未達 | #1187 skill |
| P-612 fs/exec+git | 未達 | #1187 fs/exec |
| P-615 task発話 | 部分 | #1187 task発話 |
| P-904 AI承認理由 | 未達 | #1187 audit AI |
| P-909 offlineゼロ | 未達 | #1187 offlineゼロ |

## Gate
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace
- cargo doc --workspace --no-deps

