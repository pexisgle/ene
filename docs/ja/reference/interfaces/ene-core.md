# `ene-core` インターフェース

## 役割

永続化非依存のドメイン語彙と、認知層と永続化層を分離する**ポートトレイト**。
ワークスペース内の何にも依存しない最下層です。

## 公開モジュール

モジュールはすべて private で、公開面は下記の再エクスポート（+ port モジュール）
です。

## 主要な公開型

| 領域 | 項目 |
|---|---|
| 感情 | `AffectState`（PAD: valence/arousal/dominance・trust・affinity・irritation・curiosity・fatigue）、`DiscreteEmotion`、`PendingAffectProposal` |
| 約束 | `Commitment`・`NewCommitment`・`CommitmentStatus`・`ActiveCommitmentPrompt` |
| キーファクト | `KeyFact` |
| 型付きメモリ | `MemoryItem`・`NewMemoryItem`・`MemoryKind`（10 種）・`MemoryStatus`・`MemoryScope`・`MemorySource`・`MemoryConfidence`・`MemorySalience`・`MemoryEdit`・`MemorySearchOptions`・`HybridSearchWeights`・`ScoredMemory`・`MemoryScoreBreakdown`・`MemoryOutcome`・`AffectAnnotation`・`ContradictionKeyMatch`・`ForgettingPolicy`・`Query`・`TimeRange`・`GatheredCandidate`・`MemoryCandidateSource`・`MemoryJournalListOptions` |
| 保留候補 | `PendingCandidate`・`PendingCandidateStatus`・`PendingCandidateEdit`・`NaturalDecayReport` |
| 保留書き込み | `PendingMemoryWrite`・`PendingMemoryWriteStatus` |
| スケジュール | `Schedule`・`NewSchedule`・`ScheduleKind`・`ScheduleAction`・`ScheduleRun`・`ScheduleRunStatus`・`ScheduleConfirmation`・`ScheduleError`・`first_run_at`・`next_occurrence_after` |
| スパン | `NewMemorySpan`・`ActiveSceneSummaryRow` |
| ワークスペース | `NewWorkspaceChunk`・`WorkspaceChunkHit`・`WorkspaceFileRow`・`WorkspaceIndexStatus`・`WorkspaceSearchQuery` |
| ポート | `MemoryPort`・`EmbeddingStorePort`・`ToolFailureSignalPort`・`WorkspaceDocumentPort`（+ エラー型） |

## ポートトレイト（リファクタリングの継ぎ目）

| トレイト | 契約 | 実装 |
|---|---|---|
| `MemoryPort` | 型付きメモリ CRUD・感情状態・約束・保留候補・想起検索 | `ene_store::MemoryStore`、`ene-mind` テストのテストダブル |
| `EmbeddingStorePort` | メモリ/ツール RAG のベクトル永続化 | `ene_store::MemoryStore` |
| `WorkspaceDocumentPort` | ワークスペース文書インデックス CRUD | `ene_store::MemoryStore` |
| `ToolFailureSignalPort` | RAG の負例ゲート用ツール失敗シグナル | `ene_store` |

## 依存関係

- 依存: 内部なし（serde・chrono・thiserror・tracing・schemars・async-trait）。
- 利用: `ene-store`・`ene-mind`・`ene-rag`・`ene-runtime`。

## リファクタリングの注目点

- **追加**は低リスク（全員が見えているため）。
- **変更**（既存型・ポートメソッド）は高リスク。store の SeaORM 変換・mind
  の認知ロジック・runtime のストリーミング経路に波及します。デフォルト付きの
  フィールド追加を優先してください。
- `MemoryKind::WorldState` は予約済みで現在は到達不能 — プロデューサーが
  意図的に拒否しています。設計なしにプロデューサーを追加しないでください。
- 分離作業を計画するときはここを見るべきです。mind が新しく必要とする
  永続化能力は `ene-store` から import するのではなく、このトレイトとして
  宣言してください。
