# ene 設計文書

この directory は、ene の設計の正本置き場である。Step 11 / Step 12 / Step 13 までの確定設計を、抽象度と役割に応じて配置する。巨大な summary ではなく、各 artifact への入口として使う。

## requirements と design の関係

- 製品要求の source of truth は [`docs/requirements/`](../requirements/README.md) であり、本 directory が製品挙動を追加・変更することはない。
- design は、要求を実現するための内部設計詳細（責務境界、正本、依存規則、concurrency、保存単位、interface、crate 分割、IPC 等）を定める。要求と設計の境界は requirements 側の「要件と設計の境界」に従う。
- 既存実装、Git 履歴、参考資料は設計根拠にしない。参考資料は非規範として扱う。

## directory の役割

- [`architecture/`](architecture/): 上位 Architecture。要求を実現する上位判断（Driver、境界、配置、責務分解、正本、依存、flow、横断制約）。
- [`critical-areas/`](critical-areas/): Step 11 Critical Area Detailed Design。後続設計が依存する 5 領域の logical contract。
- [`subsystems/`](subsystems/): Step 12 Subsystem Detailed Design。Subsystem cluster 別の詳細設計。
- [`concrete/`](concrete/): Step 13 Concrete Design。識別・保存・concurrency・interface・crate 分解・IPC の具体設計。
- [`reviews/`](reviews/): 独立レビュー記録。判断履歴・検証記録であり、現行 Architecture の代替正本ではない。

## 推奨 reading order

1. 上位から読む: Architecture Drivers → System Context → Runtime Topology → Subsystem Decomposition → State Ownership → Dependency Rules → Runtime Flows → Cross-cutting。
2. 次に Step 11（`critical-areas/`）を Context Assembly → Action Execution → Targeted Deletion → Client Presence Transition → Backup / Restore の順に読む。
3. 次に Step 12（`subsystems/`）を読む。5 artifact は相互に参照し合う。
4. 次に Step 13（`concrete/`）を Correspondence & Identity → Persistence / Recovery → Concurrency → Interface Boundaries → Crate / Module 分解 → Host↔Client IPC の順に読む。
5. `reviews/` は必要に応じて参照する。現行判断の正本としては使わない。

## artifact 一覧

### architecture/

- [Architecture Drivers](architecture/architecture-drivers.md): 要件から導出した設計圧力（AD-01〜15）と Requirement Issue 記録。
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

`subsystems/` 内では `subsystem-` prefix を外している（整理時に rename。旧名→新名の対応は下表のとおり）。

- [個体継続・対話調整 / 作業遂行・実行管理 / 経験に基づく認識・学習](subsystems/companion-task-learning.md)（旧 `subsystem-companion-task-learning.md`）
- [権限・利用制約 / 認証秘密の保護 / Action実行・拡張受入 / 推論利用](subsystems/permission-credential-execution-inference.md)（旧 `subsystem-permission-credential-execution-inference.md`）
- [Client接続・存在調停 / Owner入出力・体験提示 / 共有観測・関連付け](subsystems/client-presence-io-observation.md)（旧 `subsystem-client-presence-io-observation.md`）
- [内部データ保全・消去](subsystems/data-preservation-erasure.md)（旧 `subsystem-data-preservation-erasure.md`）
- [Character構成・配布](subsystems/character-distribution.md)（旧 `subsystem-character-distribution.md`）

### concrete/

- [対応関係・識別](concrete/correspondence-identity.md)（CI）: Step 13 の最初の具体設計。identity / revision / generation / correlation / boundary token。
- [Persistence / 保存単位 / Recovery](concrete/persistence-recovery.md)（PR）
- [Concurrency / Race Control / Stale-result Acceptance](concrete/concurrency-control.md)（CCT）
- [Subsystem Interface Boundary / Command・Query・Result Contract](concrete/interface-boundaries.md)（IB）
- [Crate / Module 分解と依存方向](concrete/crate-module-decomposition.md)（CM）
- [Host↔Client IPC / wire protocol](concrete/host-client-ipc.md)

### reviews/

- [Architecture Drivers Review](reviews/architecture-drivers-review.md)
- [Architecture Review #1](reviews/architecture-review-1.md)
- [Architecture Review #2](reviews/architecture-review-2.md)
- [Step 4 前独立レビュー](reviews/pre-state-ownership-review.md)

レビュー本文は歴史的記録として維持する。レビュー中の旧 `docs/design/<file>.md` 表記は当時の path の引用であり、現行配置への読み替えは本 README の対応表で行う。ナビゲーションとして機能する Markdown link target のみ現行配置へ更新している。

## 現在の設計進捗

- Upper Architecture: COMPLETE
- Critical Area Detailed Design (Step 11): COMPLETE
- Subsystem Detailed Design (Step 12): COMPLETE
- Concrete Design (Step 13): COMPLETE
- Next: Walking Skeleton

## precedence / conflict rule

1. `docs/requirements/` が製品要求の source of truth。上位・下位を問わず、設計は要求にない製品挙動を追加しない。
2. `architecture/` の artifact は、その要求を実現する上位設計（上位 contract）である。
3. `critical-areas/` / `subsystems/` / `concrete/` は上位 contract を具体化するものであり、黙って上位 contract を変更しない。Step 13 の各 artifact が宣言する固定前提（fixed premise）を守る。
4. 下位 artifact と上位 artifact に矛盾が見つかった場合、下位側で意味を上書きせず Issue として扱う。各 artifact の Issue 節に報告し、設計で吸収しない。
5. `reviews/` は判断履歴・検証記録であり、統合済みの現行 Architecture の代替 source of truth ではない。レビューの提案を再採用するのではなく、現行 architecture へ統合された判断に従う。
