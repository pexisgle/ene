# Stage 5: クライアントのライフサイクルとホストでの作業継続

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 5 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

Client を閉じても Host-only Task を継続し、再接続先へ未伝達の進捗・結果を提示します。Host restart で失われた実行は自動再開せず、保存済み facts の確認と明示 resume によって同じ Task の新 revision・新 delegation として続けます。

## 設計の入口

| 決定事項 | 所有する文書 |
|---|---|
| 製品としての継続・提示・明示再開 | [requirements §Task](../../requirements/requirements.md) |
| Host-only / Client-dependent の境界 | [Presence Transition](../../design/critical-areas/client-presence-transition.md) |
| connection phase、incarnation、wire retry、read / resume / ACK | [Host-Client IPC](../../design/concrete/host-client-ipc.md) |
| Task / Delegation / 未伝達の identity | [Correspondence Identity](../../design/concrete/correspondence-identity.md) |
| resume command / outcome / source、report の責務 | [Interface Boundaries](../../design/concrete/interface-boundaries.md) |
| 保存項目、source of truth、startup、lifecycle | [Persistence / Recovery](../../design/concrete/persistence-recovery.md) |
| commit 順序、launch 登録、connection/presence、ACK race | [Concurrency Control](../../design/concrete/concurrency-control.md) |
| E2E の期待結果 | [acceptance S5-01〜24](../../requirements/acceptance.md) |

## 実装スライス

前の gate が揃う前に後続の実行経路を enable しません。各行は必要に応じてさらに小さな stacked PR に分割します。

以下の OS transport とその検証は Stage 5 完了時点の実装範囲です。通常 Client channel の現行設計は [IPC 第10節](../../design/concrete/host-client-ipc.md#10-transport) の WSS 統一に更新しました。置換実装と両 OS の回帰は [Stage 7 A2](stage-7.md#a2-通常-client-通信の-wss-統一未実装) で行い、Stage 5 の既存検証を WSS の検証済み証拠として扱いません。

| 順 | 実装範囲と責務 | 完了条件 |
|---|---|---|
| A | `ene-core::conn/serve` と `ene-ctl::client` の connection phase、current install、terminal supersede、close admission。Client boot incarnation、descriptor と pairing identity の分離。OS transport と単一 Host lock | #1384/#1385/#1387/#1389 の回帰を接続入口で再現して通す。Linux socket と Windows named pipe が同じ認証/currentness を使う。startup 前の writer 排他、二つの Host、旧 socket の handshake 再入場拒否を検証する |
| B | `ene-presence` と `ene-store::presence` の AU7、hint、startup 復旧。Round・receipt invalidation と Host-owned runner の切断独立性 | auth/close と presence commit の race、通常切断と restart の違い、Stopped/NoActive/InTransition の復旧表を満たす。Client を落としても受理済み Host-only Task は続き、Client-dependent admission は止まる |
| C | `ene-companion` の未伝達 source/status、`ene-store` の親 fact と同時登録。Task/Action/History の bounded report query。startup sealed-result reconciliation と read-only open の分離 | producer の commit/crash で通知漏れ・重複登録なし。Unknown を再提示対象として読み、report query は durable mutation をしない。#1561 で設計矛盾を解消する |
| D | `ene-presentation`、`ene-api`、Host/Client の未伝達購読・new Round/receipt・presentation ACK。再接続時の Task list/selection を既存 owner query に接続 | 切断中 completion の自動提示、ACK 喪失、51件以上 backlog、新着競合、buffer 満杯、stale receipt を通す。provider 不調でも report と管理操作を使える |
| E | `ene-task` の `ResumeTaskCommand` / `TaskResumeOutcome`、AU17、adopted instruction source の History / first-party activity 解決。既存 `TaskExecutionRegistry` の launch reservation と runner を接続 | 同じ Task r+1・新 delegation を不可分に受理し、旧 delegation を起動しない。Unknown/terminal/running/stale/欠如/枯渇を区別する。commit 後・spawn 前 crash でも自動再実行ゼロ |
| F | 両 OS の Host integration・first-party Client E2E と Stage 4 regression | S5-01〜24 と各 OS の実 transport subset、workspace test / clippy / fmt / docs を完了する |

## 実装上の禁止事項

Stage 5 のためだけに次を新設しません。

- 専用 recovery manager / session / workflow
- Task recovery generation
- durable current-delegation / needs-resume / launch queue
- report 用 Task snapshot

導出できない伝達状態と既存 source/hint の相関だけを正しい owner に追加します。Remote network、Voice、Observation、Computer Use、Schedule、全域 Restore、Targeted Deletion 本体は後続 Stage に残します。

## 完了時点の追跡

- #1384 / #1385 / #1387 / #1389 は Stage 5 の実装と regression 確認で解消しました。
- 設計間の不一致は [#1561](https://github.com/pexisgle/ene/issues/1561) で整理しました。
- #1390（unknown wire variant）、#1508（learning correction interface）、#1530（`ScrubbedText` 構築境界）は Stage 5 の scope 外の follow-up として切り分けました。現在状態は GitHub Issue を source of truth とします。

## 完了条件

切断・再接続・Host restart・明示 resume・遅延 result・presentation failure を通じて durable fact を失わず、無断の新規 work と二重実行を起こさないこと。詳細な履歴は PR / Issue に残し、`PROGRESS.md` は milestone index に留めます。
