# `ene-mind` インターフェース

## 役割

認知エンジン: プロンプト構成・想起・メモリ書き込み・感情・プロアクティブ
発話・セッション・約束。ワークスペース最大の公開面です。`ene-runtime` と
テストがライフサイクル DTO 経由で駆動するためです。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `engine` | `CognitionEngine` ファサード（`new`・`before_turn`・`sync_character_memories` など） |
| `lifecycle` | ターン DTO: `TurnContext`・`PreTurnOutput`・`PostTurnInput`・`ComposedPrompt`・`HistoryEntry`・`PromptPacketMeta`・`interruption_note` |
| `character` | `CharacterProcessor`・アイデンティティカーネルコンパイル・lorebook 注入 |
| `session` | `ConversationSession`・`SessionId`・`CardName`・分割（`SplitResult`・`TopicBoundaryTracker`）・パフォーマンスマーカー解析 |
| `recall` | `RecallPlanner`・`RecallPlan`・`RecalledMemory`・`MemoryRecallCache`・ハイブリッド実行・多様化 |
| `memory_writer` | `MemoryWriter`・`MemoryArbiter`・判断/仲裁型・忘却・自己内省パイプライン・ツール接地 |
| `emotion` | `EmotionEngine`・`AffectProposal`・`TurnAffectInput` |
| `context` | `ContextManager`・`ContextBudget`・圧縮（`CompressionResult`・`CompressionLevel`・`execute_compression`・`pack_prompt`） |
| `prompt_packet` | `PromptPacket`・`PromptSection`・`PromptSectionKind`（固定描画順の 16 種） |
| `output` | `OutputArbiter`・`PerformanceCue`・`CueSource`・`PerfKind`・`MotionLayer`・表情判断型 |
| `proactive` | プロアクティブ判断パイプライン型（`ProactiveDecision`・`ProactiveObservation`・ゲート・静音時間・ワールド状態） |
| `commitments` | `CommitmentLedger`・`CommitmentSyncContext` |
| `summarizer` | `summarize_conversation`・`ConversationSummaryResult` |
| `config` / `error` | `MindConfig`（+ サブ設定）・`CognitionError`・`MindError` |

## 主要な再エクスポート

- コンシューマ向けに `ene-core` からメモリ型を再エクスポート
  （`Commitment`・`ActiveCommitmentPrompt` など）。
- 設定セクション型（`MindConfig`・`SessionConfig`・`ProactiveConfig`・
  `QuietHoursConfig`・`MemoryApprovalConfig` など）。

## 依存関係

- 依存: `ene-core`・`ene-config`・`ene-ai`・`ene-rag`・`ene-util`。
- 利用: `ene-runtime`・`ene-cli`・`ene-desktop`。
- 明示的に**依存しない**（本番）: `ene-runtime`・`ene-plugin-host`・
  `ene-store`（dev-dependency のみ）。

## リファクタリングの注目点

- `lifecycle` DTO は **runtime↔mind の契約**です。`ene-runtime` はこの型で
  `before_turn` / プロンプト構成 / 確定を呼びます。runtime と一緒に変更して
  ください。
- mind は `ene_core::MemoryPort` **経由でのみ**永続化に到達します。認知
  モジュールに `ene-store` 型を import しないでください。`recall` /
  `memory_writer` が最も誘惑されやすい箇所です。継ぎ目を守ってください。
- `PromptSectionKind::render_order()` は要です。セクション種別の追加は全
  プロンプトを変えます。予算ロジックは `context` にあります。
- ストリーミング連携とテストのため多数の `pub` モジュールがありますが、
  すべてが外部向けとは限りません。リファクタリング時は挙動削除より
  `pub(crate)` への絞り込みを優先してください。
- 感情（PAD）は `AffectState` + 保留提案として永続化されます。store 側の
  表現と mind のモデルは `ene-core` の型だけで同期を保ってください。
