# ene 設計文書

この directory は、ene の設計の正本置き場である。Step 11 / Step 12 / Step 13 までの確定設計を、抽象度と役割に応じて配置する。巨大な summary ではなく、各 artifact への入口として使う。

## 正本と優先順位

1. 製品挙動の正本は [`docs/requirements/`](../requirements/README.md) である。設計は要件にない製品挙動を追加・変更せず、要求と設計の境界は requirements 側の「要件と設計の境界」に従う。
2. 設計は `architecture/` → `critical-areas/` → `subsystems/` → `concrete/` の順に上位から下位へ具体化する。下位文書は上位文書の責務境界、semantic owner、identity / revision / generation の意味、保存分類、依存規則、Security / Privacy / Permission の意味を黙って変更しない。同じ層の中では、各文書が冒頭で前提として挙げる文書が先行する。
3. 文書間の矛盾、および下位設計や実装で上位文書の性質を成立させられないことが判明した場合は、下位側や実装で意味を決めず GitHub Issue として扱う。具体 mechanism が未決定であること自体は Issue ではなく、各文書の Design Freedom に属する。各文書に Issue の有無や進捗を記録する節は置かず、実装の現在地は [`docs/implementation/PROGRESS.md`](../implementation/PROGRESS.md) だけが持つ。
4. 規範となるのは現行文書へ統合された判断だけである。過去の独立レビュー記録、過去の作業指示、既存実装、Git 履歴、[参考資料](../requirements/references.md) は非規範であり、設計根拠にしない。独立レビュー記録の原文は Git 履歴（commit `b5c7990` 以前の `docs/design/reviews/`）で辿れる。

## directory の役割

- [`architecture/`](architecture/): 上位 Architecture。要求を実現する上位判断（Driver、境界、配置、責務分解、正本、依存、flow、横断制約）。
- [`critical-areas/`](critical-areas/): Step 11 Critical Area Detailed Design。後続設計が依存する 5 領域の logical contract。
- [`subsystems/`](subsystems/): Step 12 Subsystem Detailed Design。Subsystem cluster 別の詳細設計。
- [`concrete/`](concrete/): Step 13 Concrete Design。識別・保存・concurrency・interface・crate 分解・IPC の具体設計。

`critical-areas/` と `subsystems/` の文中の「要求」「対応」「区間」「保留」「受入」等は論理的な関係を表し、共通 object、永続 record、protocol、state machine、enum を指定しない。番号付きの段階は必要な前後関係を示し、直列実行の指定ではない。

## 推奨 reading order

1. 上位から読む: Architecture Drivers → System Context → Runtime Topology → Subsystem Decomposition → State Ownership → Dependency Rules → Runtime Flows → Cross-cutting。
2. 次に Step 11（`critical-areas/`）を Context Assembly → Action Execution → Targeted Deletion → Client Presence Transition → Backup / Restore の順に読む。
3. 次に Step 12（`subsystems/`）を読む。5 artifact は相互に参照し合う。
4. 次に Step 13（`concrete/`）を Correspondence & Identity → Persistence / Recovery → Concurrency → Interface Boundaries → Crate / Module 分解 → Host↔Client IPC の順に読む。

## artifact 一覧

### architecture/

- [Architecture Drivers](architecture/architecture-drivers.md): 要件から導出した設計圧力（AD-01〜15）。
- [System Context](architecture/system-context.md): 製品責任と外部環境の境界（SC-01〜10）。
- [Runtime Topology](architecture/runtime-topology.md): 実行場所、主体の寿命、接続、信頼・障害境界（RT-01〜10）。
- [Subsystem Decomposition](architecture/subsystems.md): 12 Subsystem への責務分解。
- [State Ownership](architecture/state-ownership.md)（SO）: 概念 / architectural state ownership。
- [Dependency Rules](architecture/dependency-rules.md)（DR）: 責務間の依存規則（DR-01〜12）。
- [Major Runtime Flows](architecture/runtime-flows.md)（RF）: 主要活動の時間軸検証（RF-01〜08）。
- [Cross-cutting Design](architecture/cross-cutting.md)（CC）: Subsystem・Flow 横断の共通制約（CC-01〜07）。

### critical-areas/

- [Context Assembly](critical-areas/context-assembly.md): 情報選択・変換・Provider 適応の一般契約。
- [Action Execution](critical-areas/action-execution.md): 認可判断から実作用・確定度・報告対応付けまでの一般契約。
- [Targeted Deletion](critical-areas/targeted-deletion.md): Privacy / Security 目的の全域消去と再保存防止の一般契約。
- [Client Presence Transition](critical-areas/client-presence-transition.md): presence 判断・切替区間・Client 依存作用の区切りの一般契約。
- [Backup / Restore](critical-areas/backup-restore.md): 復元範囲・正本切替・再有効化の一般契約。

### subsystems/

- [個体継続・対話調整 / 作業遂行・実行管理 / 経験に基づく認識・学習](subsystems/companion-task-learning.md)
- [権限・利用制約 / 認証秘密の保護 / Action実行・拡張受入 / 推論利用](subsystems/permission-credential-execution-inference.md)
- [Client接続・存在調停 / Owner入出力・体験提示 / 共有観測・関連付け](subsystems/client-presence-io-observation.md)
- [内部データ保全・消去](subsystems/data-preservation-erasure.md)
- [Character構成・配布](subsystems/character-distribution.md)

### concrete/

- [対応関係・識別](concrete/correspondence-identity.md)（CI）: identity / revision / generation / correlation / boundary token。
- [Persistence / 保存単位 / Recovery](concrete/persistence-recovery.md)（PR）
- [Concurrency / Race Control / Stale-result Acceptance](concrete/concurrency-control.md)（CCT）
- [Subsystem Interface Boundary / Command・Query・Result Contract](concrete/interface-boundaries.md)（IB）
- [Crate / Module 分解と依存方向](concrete/crate-module-decomposition.md)（CM）
- [Host↔Client IPC / wire protocol](concrete/host-client-ipc.md)
