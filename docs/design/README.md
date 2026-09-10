# ene 設計文書

ene の内部設計の正本。巨大な summary ではなく、目的から各 artifact へ辿る入口として使う。

## 正本と優先順位

1. 製品挙動の正本は [`docs/requirements/`](../requirements/README.md) である。設計は要件にない製品挙動を追加・変更せず、要求と設計の境界は requirements 側の「要件と設計の境界」に従う。
2. 設計は `architecture/` → `critical-areas/` → `subsystems/` → `concrete/` の順に上位から下位へ具体化する。下位文書は上位文書の責務境界、semantic owner、identity / revision / generation の意味、保存分類、依存規則、Security / Privacy / Permission の意味を黙って変更しない。同じ層の中では、各文書が冒頭で前提として挙げる文書が先行する。
3. 文書間の矛盾、および下位設計や実装で上位文書の性質を成立させられないことが判明した場合は、下位側や実装で意味を決めず GitHub Issue として扱う。具体 mechanism が未決定であること自体は Issue ではなく、各文書の Design Freedom に属する。各文書に Issue の有無や進捗を記録する節は置かず、実装の現在地は [`docs/implementation/PROGRESS.md`](../implementation/PROGRESS.md) だけが持つ。
4. 規範となるのは現行文書へ統合された判断だけである。過去の独立レビュー記録、過去の作業指示、既存実装、Git 履歴、[参考資料](../requirements/references.md) は非規範であり、設計根拠にしない。独立レビュー記録の原文は Git 履歴（commit `b5c7990` 以前の `docs/design/reviews/`）で辿れる。

## 目的別索引

工程順に全部読むことは通常の前提にしない。触る目的から正本へ辿る。

- **保存・復旧・競合:** [Persistence / Recovery](concrete/persistence-recovery.md)（PR）、[Concurrency](concrete/concurrency-control.md)（CCT）
- **通信（Host↔Client）:** [Host↔Client IPC](concrete/host-client-ipc.md)
- **Learning / 会話 / Task:** [個体継続・対話調整 / 作業 / 認識・学習](subsystems/companion-task-learning.md)、[Context Assembly](critical-areas/context-assembly.md)
- **権限 / 実行 / 秘密 / 推論:** [権限・秘密・実行・推論](subsystems/permission-credential-execution-inference.md)、[Action Execution](critical-areas/action-execution.md)
- **削除 / 保全 / Restore:** [Targeted Deletion](critical-areas/targeted-deletion.md)、[内部データ保全・消去](subsystems/data-preservation-erasure.md)、[Backup / Restore](critical-areas/backup-restore.md)
- **presence / 入出力 / 観測:** [Client Presence Transition](critical-areas/client-presence-transition.md)、[接続・入出力・観測](subsystems/client-presence-io-observation.md)
- **Character:** [Character構成・配布](subsystems/character-distribution.md)
- **crate / interface:** [Crate / Module 分解](concrete/crate-module-decomposition.md)（CM）、[Interface Boundaries](concrete/interface-boundaries.md)（IB）
- **識別・対応:** [対応関係・識別](concrete/correspondence-identity.md)（CI）
- **所有・依存・配置:** [State Ownership](architecture/state-ownership.md)（SO）、[Dependency Rules](architecture/dependency-rules.md)（DR）、[System Context](architecture/system-context.md)、[Runtime Topology](architecture/runtime-topology.md)、[Subsystem Decomposition](architecture/subsystems.md)
- **横断制約・時間軸・設計圧力:** [Cross-cutting](architecture/cross-cutting.md)（CC）、[Runtime Flows](architecture/runtime-flows.md)（RF）、[Architecture Drivers](architecture/architecture-drivers.md)（AD）

`critical-areas/` と `subsystems/` の文中の「要求」「対応」「区間」「保留」「受入」等は論理的な関係を表し、共通 object、永続 record、protocol、state machine、enum を指定しない。番号付きの段階は必要な前後関係を示し、直列実行の指定ではない。

## artifact 一覧

契約 ID の所在。本文は再掲しない。

- AD-01〜15: [Architecture Drivers](architecture/architecture-drivers.md)
- SC-01〜10: [System Context](architecture/system-context.md)
- RT-01〜10: [Runtime Topology](architecture/runtime-topology.md)
- SO: [State Ownership](architecture/state-ownership.md)
- DR-01〜12: [Dependency Rules](architecture/dependency-rules.md)
- RF-01〜08: [Runtime Flows](architecture/runtime-flows.md)
- CC-01〜07: [Cross-cutting Design](architecture/cross-cutting.md)
