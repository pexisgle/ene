# Stage 4: 作業用エージェント (Task Agent) とファイル操作タスク

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 4 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

Owner が指定した Workspace を境界として、Task Agent が安全にファイル作業を実行し、会話から作成・steering・cancel・進捗確認・結果報告まで行える縦断経路を完成させます。

## Stage 4 内の前提

Task Agent の provider send を完成させる前に、Stage 6 全体ではなく、**送信 currentness に必要な最小 erasure-condition foundation** を先行して導入します。

この foundation は次だけを持ちます。

- preservation-owned canonical current erasure-condition state と durable store
- 送信 source 相関 (`data_use`) に対する coverage 判定
- AU14 推論試行 claim と同一 transaction での currentness compare
- `DataUseHeld`
- inference attempt と `data_use` の durable 相関

active condition が 0 件の場合も canonical store を実際に照会した結果として送信を許可し、`NoDeletion` や generation 0 のような producer のない sentinel を作りません。Stage 6 はこの foundation を置き換えず拡張します。

## 実装スライス

| 順 | 実装範囲 | 完了条件 |
|---|---|---|
| A | minimal erasure-currentness foundation | current condition の persistence、coverage gate、`DataUseHeld`、attempt の `data_use` 相関が成立する |
| B | adopted-instruction History bounded read、`TaskInstructionSource`、body resolution、correspondence validation、context assembly、credential scrub と actual provider send | A の currentness gate を必ず通り、stale / held premise では provider I/O を開始しない |
| C | Cancel | owner gate を通じて cancel を durable lifecycle へ反映する |
| D | autonomous Task Agent ↔ Action loop | closed-world の file operation だけで Task Agent が継続実行でき、Action certainty を durable facts として扱う |
| E | Host 結合の Stage 4 E2E | report 生成、path traversal 拒否、restart 読み戻し、cancel、遅延 result を通す |
| F | 会話・第一者管理経路からの Task control | `ProposeTaskCommand`、steering、cancel、report を既存 Task owner 境界へ接続し、会話入力だけを起点に acceptance 4.3 / 4.4 / 4.5 / 4.7 を通す |
| G | terminal failure (`Failed`) producer | 作業担当が確定できる terminal failure を lifecycle の canonical fact として保存する |

A と B を同じ PR にまとめる必要はありません。レビューしやすい最小 stacked PR を優先しますが、B を A より先に provider-send-enabled として merge しません。E の完了だけで Stage 4 完了とはせず、F と G まで通します。

## 重要な実装境界

- Workspace association は第一者管理経路の Owner 選択 premise からのみ成立させる。
- 会話起点の書き込みは、依拠する Owner message の currentness と同じ不可分区間で commit する。
- Task 作成 + delegation の受理後は serving composition の launcher が既存 Task Agent runner を background 起動する。
- steering 後は new revision 用の新しい delegation を作り、同じ launcher を使う。
- `ManagementIntentKind::CancelTask` は `HostHandle::cancel_task` へ接続する。
- report は別 snapshot を新設せず、既存 Task / Action facts から組み立てる。
- shell 実行や外部 network など、Stage 4 の closed-world tool 語彙外の作用は追加しない。

## 完了条件

- 指定 Workspace 内のファイルを読み、新しい report を生成できる。
- Workspace 外アクセスを構造的に拒否できる。
- 会話から Task 作成・steering・cancel・進捗確認・結果報告まで行える。
- `Unknown` な外部作用を自動再実行しない。
- Stage 4 acceptance と Linux / Windows CI を通す。

詳細な実装履歴は GitHub Issues / Pull Requests に残し、この文書を進捗ログにはしません。
