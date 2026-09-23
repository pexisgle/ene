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

まず [Stage 7 A2](stages/stage-7.md#a2-通常-client-通信の-wss-統一未実装) の WSS 統一を進めます。wire の未決事項は [基盤計画](follow-ups/foundation.md) と照合し、既存の確認 channel や Body 投影まで WSS に混ぜません。公式 VRM、物理 HiDPI、指定 OS と統合後の再検証は [Desktop 計画](follow-ups/desktop.md) と Stage 7 F に従います。

次の製品機能は、Schedule だけを先に実装するのではなく、**音声で Task を依頼・変更・確認・中止・再開する最小経路**を優先します。Stage 10 の番号は変えず、[D0 の要件・設計更新](stages/stage-10.md#2-d0-要件と設計を先に確定する) を A2 と並行して進めます。通常の Task UX と提示確認の意味は製品要件・受け入れ条件・内部設計が定め、この実装ガイドだけでは変更しません。

共通境界の確定後に Voice・対象特定・Workspace/権限の対話経路を進め、報告の提示確認まで接続してから作業 GUI を削除します。具体的な依存順は Stage 10 が所有します。音声の了承だけを第一者確認とする変更、既存の安全操作や成果物確認機能を失わせる削除は行いません。

### 4.2 後続機能と着手条件

| Stage / 領域 | 機能 | 前提・計画 |
|---|---|---|
| 10（優先） | Voice、会話での Task 操作・報告、不要な作業 GUI の削除 | D0、並列化、実装・統合 gate は [Stage 10 計画](stages/stage-10.md) |
| Task / Execution 拡張 | Web 取得・MCP・PC 操作 | 対象ごとに設計・実装を分け、既存 Action / permission / Task 境界を利用。PC 操作はユーザー介入による停止を検証 |
| 8 | 定期 schedule 実行 | Task・permission・usage と対話による設定/報告。各回は新しい Task、Host 停止中の回はスキップ |
| 9 | backup / restore / Host 自動起動 | preservation・管理/復旧経路。音声や新しい実行ツールに依存しない部分は独立して計画可能 |
| 11 | Observation | Client の観測・permission・プライバシー境界。PC 操作用の観測と共有する境界を設計時に調整 |
| 12 | group conversation・Companion 間交流・並行作業の調整 | 会話・Memory・Task 委任。対象特定と実行側の協調を区別 |
| 13 | global memory・skill learning・emotion / relationship | Memory・learning。Skill の実装だけで Stage 全体を完了としない |
| 14 | remote Client | device authentication・backup / restore・WSS 基盤。ローカル WSS の完成だけで remote 接続を提供済みにしない。[起動時の接続先選択](follow-ups/desktop.md)もここで接続する |
| 15 | character edit・package distribution | character management・settings UI。公式同梱 VRM の入手を汎用 editor の実装へ広げない |
| 16 | multi-provider・automatic fallback | inference・cost limit。結果不明の外部作用を自動再実行しない |

Stage 8 / 9 / 11〜16 の詳細計画は未作成です。着手時に `stages/stage-N.md` を追加し、要件への対応・最小経路・失敗経路・完了条件を具体化します。Web / MCP / PC 操作もそれぞれ計画を分けます。これら全ての完成を Stage 10 の音声 Task 最小経路の gate には含めません。

### 4.3 後続作業の実装計画

Stage 完了の履歴を変えずに残る作業を、Issue に依存しない計画として管理します。各文書に作業順、上位契約の変更 gate、失敗経路、検証を置きます。着手時には現行の source と要件・設計を再確認します。

| 領域 | 実装計画 | 接点 |
|---|---|---|
| Learning / Memory | [learning.md](follow-ups/learning.md) | Stage 3 / Stage 7 C1。学習設定 → 形成方式、対象指定訂正の設計整合 |
| 費用・provider 障害 | [usage.md](follow-ups/usage.md) | Stage 6 / Stage 7 C3。上限の設計変更 → 価格・画面。障害分類は独立 |
| Desktop / Body / Host 寿命 | [desktop.md](follow-ups/desktop.md) | Stage 7 B/D/F、remote 接続は Stage 14。現行設計と違う寿命・起動方式は先に要件・設計を更新 |
| Wire・共通型・起動設定 | [foundation.md](follow-ups/foundation.md) | Stage 7 A2 / Stage 10。変更する境界の直前に判断し、独立した整理は並行 |

## 5. 共通 quality gate

正確な command と lint policy は [`AGENTS.md`](../../AGENTS.md) と CI workflow を source of truth とします。実装ガイド側では、各 PR / Stage が満たす性質だけを管理します。

### 各 PR

- 変更した behavior を固定する unit / integration test がある。
- crash、通信断、二重実行、stale premise など、変更に関係する failure path を検証する。
- credential や削除対象本文を log / error / prompt に漏らさない。
- durable state を変更する場合、restart 後の読み戻しまたは recovery を検証する。
- format / lint / test / build の repository CI を通す。

Docs-only PR は参照先、上位契約との整合、計画の依存関係、差分が文書だけであることを検証します。Rust テストや実機 probe を実施していなければ、その旨を記載し、過去の結果で代用しません。

### Stage 完了

- [`acceptance.md`](../requirements/acceptance.md) の該当 scenario を満たす。
- 後続 Stage が依存する public boundary が設計どおり固定されている。
- Windows / Linux の対象経路を両方検証する。
- 未解決 finding は blocker と non-blocking follow-up を分離し、`PROGRESS.md` と対応する計画を同期する。
