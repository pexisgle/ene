# ene 実装ガイド

ステータス: **開発実装用ガイド**

このディレクトリは、ene の**実装順・PR 分割・完了 gate**を管理します。製品の振る舞いや内部設計そのものを定義する場所ではありません。

- **仕様・要件**: [`docs/requirements/`](../requirements/README.md)
- **アーキテクチャ・内部設計**: [`docs/design/`](../design/README.md)
- **開発用コマンド・Rust 規約**: [`AGENTS.md`](../../AGENTS.md)
- **現在の進捗**: [`PROGRESS.md`](PROGRESS.md)

> [!IMPORTANT]
> 要件や設計と実装ガイドが矛盾した場合は、常に要件・設計を優先します。実装中に設計上の曖昧さや矛盾が見つかった場合は、コードで意味を決めず、Issue または docs-only PR で先に契約を確定してください。

## 1. 開発の進め方

### 1.1 縦断スライスを優先する

モジュールや crate を上から順に作り込むのではなく、ユーザーから見て最初から最後まで動く最小経路を先に通します。

```text
入力 → Host 受付 → Companion 判断 → 権限確認 → AI 推論 → 応答の提示 → 履歴保存
```

動く土台ができてから、Memory、Task、recovery などを段階的に追加します。将来必要そうという理由だけで、使われない共通 manager、巨大 coordinator、汎用 abstraction を先行して作りません。

### 1.2 Stacked PR を基本にする

1つの巨大 PR ではなく、レビュー可能で各段階が build / test 可能な小さな PR を積み重ねます。

- **依存する変更**: 前の PR の branch 上に積み、下から順に merge する。
- **独立した変更**: 同じ安定した base から並列化してよい。
- **設計変更**: production code と混ぜず、先に docs-only PR で確定する。
- **PR 本文**: 対応する要件・設計、含む範囲、意図的に含めない範囲、検証内容を明記する。

各 Stage の具体的な slice 順や parallel gate は、その Stage の個別文書が所有します。

### 1.3 後方互換性は考慮しない

ene の新実装では旧実装との backward compatibility、migration shim、legacy workaround を設計しません。現在の要件・設計を直接満たす、最も単純で堅牢な実装を優先します。

### 1.4 実装文書の責務を分ける

| 文書 | 責務 |
|---|---|
| `README.md` | 全 Stage に共通する開発原則、文書構造、Stage index、後続機能と Issue の対応、共通 quality gate |
| [`PROGRESS.md`](PROGRESS.md) | current milestone、completed stage、blocker、次の作業の短い index |
| `stages/stage-N.md` | その Stage 固有の目的、前提、実装 slice、依存関係、並列化条件、完了 gate |
| GitHub Issues / Pull Requests | 個別 finding、現在の状態、実装履歴、review、実際の stack / commit の記録 |

Stage 文書は**実装方法を具体化する場所**であり、新しい製品 behavior や authority を定義する場所ではありません。Stage 文書を詳しくする場合も、要件・設計への参照を増やし、同じ契約本文をコピーしないことを優先します。

完了済み Stage の文書は実装計画・境界の参照として残しますが、日々の進捗ログにはしません。進捗は `PROGRESS.md`、詳細履歴は Issues / PRs に残します。設計の確定、実装の merge、技術成立 probe、製品 acceptance は別々に判定し、未マージ PR や古い報告を現在の `main` の合格として扱いません。

## 2. Stage index

現在状態は [`PROGRESS.md`](PROGRESS.md) を参照します。Stage 番号は参照を保つための識別子であり、今後の着手順は §4 と各 Stage の前提で決めます。

| Stage | テーマ | 実装計画 |
|---|---|---|
| 0 | リポジトリとビルド基盤 | [stage-0.md](stages/stage-0.md) |
| 1 | 最小限の共通規約と基盤 | [stage-1.md](stages/stage-1.md) |
| 2 | セットアップとテキスト会話 | [stage-2.md](stages/stage-2.md) |
| 3 | Experience Summary / Memory | [stage-3.md](stages/stage-3.md) |
| 4 | Task Agent とファイル操作 | [stage-4.md](stages/stage-4.md) |
| 5 | Client lifecycle と Host での作業継続 | [stage-5.md](stages/stage-5.md) |
| 6 | Targeted Deletion、usage / cost、credential safety | [stage-6.md](stages/stage-6.md) |
| 7 | 管理画面、desktop avatar、最初の acceptance | [stage-7.md](stages/stage-7.md) |
| 10 | Voice と会話中心の Task 操作 | [stage-10.md](stages/stage-10.md) |

## 3. 並列化の共通原則

- public interface や schema に依存関係がある場合は、最小の prerequisite PR を先に merge してから並列化する。
- semantic owner が異なり、同じ authority / schema を同時に変更しない slice は同じ stable base から並列化してよい。
- test harness、CI、UI rendering なども、製品 behavior を先取りしない範囲で feature 実装と並行してよい。
- 具体的な「何と何を並列に進められるか」は各 `stages/stage-N.md` に書き、README に Stage 固有の dependency graph を持ち込まない。

## 4. 最初の milestone 後の roadmap

後続機能の計画は、個別 Stage 文書で具体化してから実装します。計画の存在は設計確定・実装済みを意味しません。公式アセットや指定 OS での検証待ちは独立した設計・開発の停止理由にしませんが、Stage 7 / Milestone 1 の gate は免除しません。

### 4.1 直近の進め方

まず [Stage 7 A2](stages/stage-7.md#a2-通常-client-通信の-wss-統一未実装) の WSS 統一を [#1705](https://github.com/pexisgle/ene/issues/1705) で進めます。wire の未決事項は §4.3 と照合し、既存の確認 channel や Body 投影まで WSS に混ぜません。公式 VRM [#1651](https://github.com/pexisgle/ene/issues/1651)、物理 HiDPI [#1678](https://github.com/pexisgle/ene/issues/1678)、指定 OS と統合後の再検証は、closeout [#1706](https://github.com/pexisgle/ene/issues/1706) に集約します。

次の製品機能は、Schedule だけを先に実装するのではなく、[#1686](https://github.com/pexisgle/ene/issues/1686) の**音声で Task を依頼・変更・確認・中止・再開する最小経路**を優先します。Stage 10 の番号は変えず、[D0 の要件・設計更新](stages/stage-10.md#2-d0-要件と設計を先に確定する) を A2 と並行して進めます。現行要件の作業 GUI・画面提示契約を、この実装ガイドだけで変更してはいけません。

共通境界の確定後に Voice・対象特定・Workspace/権限の対話経路を進め、報告の提示確認まで接続してから作業 GUI を削除します。具体的な依存順は Stage 10 が所有します。音声の了承だけを第一者確認とする変更、既存の安全操作や成果物確認機能を失わせる削除は行いません。

### 4.2 機能と追跡先

| Stage / 領域 | 機能 | 前提・追跡先 |
|---|---|---|
| 10（優先） | Voice、会話での Task 操作・報告、不要な作業 GUI の削除 | [#1688](https://github.com/pexisgle/ene/issues/1688)・[#1689](https://github.com/pexisgle/ene/issues/1689)・[#1691](https://github.com/pexisgle/ene/issues/1691)・[#1690](https://github.com/pexisgle/ene/issues/1690)・[#1687](https://github.com/pexisgle/ene/issues/1687)。D0、並列化、実装・統合 gate は [計画](stages/stage-10.md) |
| Task / Execution 拡張 | Web 取得・MCP・PC 操作 | [#1692](https://github.com/pexisgle/ene/issues/1692)。対象ごとに設計・実装を分け、既存 Action / permission / Task 境界を利用。PC 操作はユーザー介入による停止を検証 |
| 8 | 定期 schedule 実行 | Task・permission・usage と対話による設定/報告。[#1693](https://github.com/pexisgle/ene/issues/1693)。各回は新しい Task、Host 停止中の回はスキップ |
| 9 | backup / restore / Host 自動起動 | preservation・管理/復旧経路。音声や新しい実行ツールに依存しない部分は独立して計画可能 |
| 11 | Observation | Client の観測・permission・プライバシー境界。PC 操作用の観測と共有する境界は #1692 の設計時に調整 |
| 12 | group conversation・Companion 間交流・並行作業の調整 | 会話・Memory・Task 委任。[#1695](https://github.com/pexisgle/ene/issues/1695)。対象特定 #1689 と実行側の協調を区別 |
| 13 | global memory・skill learning・emotion / relationship | Memory・learning。[#1694](https://github.com/pexisgle/ene/issues/1694) は Skill の追跡であり、Stage 全体の完了条件ではない |
| 14 | remote Client | device authentication・backup / restore・WSS 基盤。[#1696](https://github.com/pexisgle/ene/issues/1696)。ローカル WSS の完成だけで remote 接続を提供済みにしない |
| 15 | character edit・package distribution | character management・settings UI。公式同梱 VRM #1651 の入手を汎用 editor の実装へ広げない |
| 16 | multi-provider・automatic fallback | inference・cost limit。結果不明の外部作用を自動再実行しない |

Stage 8 / 9 / 11〜16 の詳細計画は未作成です。着手時に `stages/stage-N.md` を追加し、要件への対応・最小経路・失敗経路・完了条件を具体化します。#1692 は Web / MCP / PC 操作それぞれの計画へ分けます。未分割の tracker を、実装可能な一枚の巨大 PR と扱いません。

#1686 は #1687〜#1696 を束ねる親 Issue として残します。D0 や Stage 10 の完了だけで親を close せず、一方で #1692〜#1696 の全実装を音声 Task 最小経路の完成条件にも含めません。

### 4.3 基盤の設計・簡素化 follow-up

以下は一括で全て片付くまで開発を止める backlog ではありません。変更する境界に関係する項目を先に解決し、独立した整理は並行できます。Issue の調査時点の行数・テスト数・API は、着手時の `main` で再確認します。

| Issue | 扱う判断・残作業 | 着手の接点 |
|---|---|---|
| [#1390](https://github.com/pexisgle/ene/issues/1390) | 未知 payload / enum のメッセージ単位拒否と接続維持 | A2 の codec 集約時に IPC §7.2 と整合。TLS 化だけでは解決しない。旧版互換レイヤーを追加する理由にはしない |
| [#1700](https://github.com/pexisgle/ene/issues/1700) | `round` / `fresh` の不正な組合せをなくし、Auto / Existing / New の採否を設計で決める | A2 と Voice の入力境界を確定する際に調整。#1697 の cleanup と分離し、DTO・Host・GUI を同じ契約に揃える |
| [#1682](https://github.com/pexisgle/ene/issues/1682) | primitive の共有範囲と、値型・所有者それぞれの保証を明確化 | 設計 → 必要な型の整理。Generation と Revision を一律に削らず、Money・固定 offset 時刻も扱う |
| [#1683](https://github.com/pexisgle/ene/issues/1683) | primitive の未使用 Serde と client の直接依存を再確認・削減 | #1682 の存廃判断を待たず評価可能。削除済みの独自テスト基盤を再作成しない |
| [#1684](https://github.com/pexisgle/ene/issues/1684) | 起動パス・CLI 本文言語・GUI 表示言語の責務、起動 JSON の存廃を設計で決める | A2 の接続準備と GUI 設定変更に合わせて調整。#1683 と client manifest の競合を避ける |
| [#1508](https://github.com/pexisgle/ene/issues/1508) | formation 内包の訂正と H-C の独立した訂正 command / outcome の整合 | 対象指定の訂正 caller を追加する前に判断。Stage 3 を再び未完了にはせず、Stage 13 の learning 設計でも確認 |

## 5. 共通 quality gate

正確な command と lint policy は [`AGENTS.md`](../../AGENTS.md) と CI workflow を source of truth とします。実装ガイド側では、各 PR / Stage が満たす性質だけを管理します。

### 各 PR

- 変更した behavior を固定する unit / integration test がある。
- crash、通信断、二重実行、stale premise など、変更に関係する failure path を検証する。
- credential や削除対象本文を log / error / prompt に漏らさない。
- durable state を変更する場合、restart 後の読み戻しまたは recovery を検証する。
- format / lint / test / build の repository CI を通す。

Docs-only PR は参照先、上位契約との整合、Issue の状態と依存、差分が文書だけであることを検証します。Rust テストや実機 probe を実施していなければ、その旨を記載し、過去の結果で代用しません。

### Stage 完了

- [`acceptance.md`](../requirements/acceptance.md) の該当 scenario を満たす。
- 後続 Stage が依存する public boundary が設計どおり固定されている。
- Windows / Linux の対象経路を両方検証する。
- 未解決 finding は blocker と non-blocking follow-up を分離し、`PROGRESS.md` と Issue の状態を同期する。
