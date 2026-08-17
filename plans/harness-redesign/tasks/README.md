# tasks/ — 仕事面

対話の外で進む「仕事」。裏層ハーネスが実行し、表層 soul がユーザーへ
伝える。**ユーザー向けの呼称は「タスク」**(D-12)。2文書。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [jobs-and-schedules.md](jobs-and-schedules.md) | 内部 workspace・job ライフサイクル・スケジュール・リマインダー・ワークフロー・artifact | P-605..P-609 |
| [skills.md](skills.md) | skill パッケージ(業界標準準拠+独自拡張)・カタログ・MCP prompts | P-610 |

## この層に効く主な決定

- **進捗はコンパニオンが語る**(D-13)。表層UIにタスクの進捗バーや
  ステータス列は置かない。一覧は詳細画面側。
- **異常終了した job は自動再開しない**(D-5)。検出・片付け・報告までが
  v1.0 の範囲。未消化の報告や質問はログから見せ、再開の判断はユーザーがする。

## 他フォルダとの接点

- job レーン → [../core/agent-loop.md §2](../core/agent-loop.md#2-レーンp-504)
- 表示の深さ → [../core/visibility.md](../core/visibility.md)
- 能動発話(会話/活動/記憶。スケジュールは含めない) → [../companion/proactive.md](../companion/proactive.md)
- spawn ごとの job workspace → [jobs-and-schedules.md §1](jobs-and-schedules.md#1-内部-workspace-概念)
- script の承認 → [../security/approval.md](../security/approval.md)
- soul の skill 参照 → [../character/package-format.md](../character/package-format.md)
