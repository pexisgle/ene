# ene 内部設計ドキュメント

このフォルダには、ene のシステム構造や内部設計（アーキテクチャ）に関する設計書をまとめています。
すべてを最初から通読する必要はありません。開発したい機能や目的に応じて、以下の「目的別インデックス」から該当ドキュメントを参照してください。

## 正本と優先順位

各ドキュメントの階層関係と優先順位は以下の通りです：

1. **最優先は「製品要件」**:
   製品としての振る舞いの基準は [`docs/requirements/`](../requirements/README.md) です。内部設計が要件にない仕様を勝手に追加・変更することはありません。
2. **設計の具体化の流れ**:
   設計は **全体アーキテクチャ (`architecture/`)** → **重要領域 (`critical-areas/`)** → **サブシステム (`subsystems/`)** → **具象設計 (`concrete/`)** の順に、全体像から具体的なモジュールへと掘り下げて記述しています。
3. **矛盾が見つかった場合**:
   設計同士の矛盾や、実装上で上位設計の前提を満たせないことが判明した場合は、その場のコードで勝手に解決せず、GitHub Issue として起票して設計方針を合意してください。

## Artifact 一覧

知りたいトピックや開発対象から、該当する設計ドキュメントへジャンプできます（目的別インデックス）：

- **データの保存・復旧・競合制御**:
  - [データの永続化と復旧 (Persistence / Recovery)](concrete/persistence-recovery.md)
  - [並行処理と排他制御 (Concurrency Control)](concrete/concurrency-control.md)
- **ホストとクライアントの通信**:
  - [Host↔Client プロトコル・IPC (Host-Client IPC)](concrete/host-client-ipc.md)
- **記憶・対話・タスク協調**:
  - [パートナー対話・タスク・学習サブシステム](subsystems/companion-task-learning.md)
  - [コンテキストの組み立て (Context Assembly)](critical-areas/context-assembly.md)
  - [記憶の検索・文脈選択 (Context Selection)](concrete/context-selection.md)
- **権限・アクション実行・推論**:
  - [権限・認証情報・実行・推論サブシステム](subsystems/permission-credential-execution-inference.md)
  - [アクション実行の安全性 (Action Execution)](critical-areas/action-execution.md)
- **データの削除・保全・バックアップ**:
  - [指定データの完全削除 (Targeted Deletion)](critical-areas/targeted-deletion.md)
  - [内部データの保全と消去サブシステム](subsystems/data-preservation-erasure.md)
  - [バックアップと復元 (Backup / Restore)](critical-areas/backup-restore.md)
- **クライアント接続・画面と音声・観測**:
  - [パートナーの存在場所とクライアント遷移 (Presence Transition)](critical-areas/client-presence-transition.md)
  - [画面・音声入出力と観測サブシステム](subsystems/client-presence-io-observation.md)
- **キャラクターパッケージ**:
  - [キャラクター構成と配布サブシステム](subsystems/character-distribution.md)
- **クレート分割・インターフェース**:
  - [クレートおよびモジュール分割方針 (Crate Decomposition)](concrete/crate-module-decomposition.md)
  - [インターフェース境界設計 (Interface Boundaries)](concrete/interface-boundaries.md)
  - [IDと対応関係の識別規則 (Correspondence Identity)](concrete/correspondence-identity.md)
- **全体システム構造・アーキテクチャ制約**:
  - [設計の推進要因 (Architecture Drivers)](architecture/architecture-drivers.md)
  - [システム全体のコンテキスト (System Context)](architecture/system-context.md)
  - [実行トポロジ (Runtime Topology)](architecture/runtime-topology.md)
  - [状態の所有権と配置 (State Ownership)](architecture/state-ownership.md)
  - [依存関係のルール (Dependency Rules)](architecture/dependency-rules.md)
  - [実行フロー (Runtime Flows)](architecture/runtime-flows.md)
  - [横断的関心事 (Cross-cutting Design)](architecture/cross-cutting.md)
  - [サブシステム分割 (Subsystems)](architecture/subsystems.md)

