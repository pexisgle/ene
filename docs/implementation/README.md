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
画面入力 → Host 受付 → Companion 判断 → 権限確認 → AI 推論 → 画面返信 → 履歴保存
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

ene の新実装では `.old/` を含む旧実装との backward compatibility、migration shim、legacy workaround を設計しません。現在の要件・設計を直接満たす、最も単純で堅牢な実装を優先します。

### 1.4 実装文書の責務を分ける

| 文書 | 責務 |
|---|---|
| `README.md` | 全 Stage に共通する開発原則、文書構造、Stage index、共通 quality gate |
| [`PROGRESS.md`](PROGRESS.md) | current milestone、completed stage、blocker、next stage の短い index |
| `stages/stage-N.md` | その Stage 固有の目的、前提、実装 slice、依存関係、並列化条件、完了 gate |
| GitHub Issues / Pull Requests | 個別 finding、実装履歴、review、実際の stack / commit の記録 |

Stage 文書は**実装方法を具体化する場所**であり、新しい製品 behavior や authority を定義する場所ではありません。Stage 文書を詳しくする場合も、要件・設計への参照を増やし、同じ契約本文をコピーしないことを優先します。

完了済み Stage の文書は実装計画・境界の参照として残しますが、日々の進捗ログにはしません。進捗は `PROGRESS.md`、詳細履歴は Issues / PRs に残します。

## 2. Stage index

現在状態は [`PROGRESS.md`](PROGRESS.md) を source of truth とし、ここでは Stage の詳細を重複して記載しません。

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

## 3. 並列化の共通原則

- public interface や schema に依存関係がある場合は、最小の prerequisite PR を先に merge してから並列化する。
- semantic owner が異なり、同じ authority / schema を同時に変更しない slice は同じ stable base から並列化してよい。
- test harness、CI、UI rendering なども、製品 behavior を先取りしない範囲で feature 実装と並行してよい。
- 具体的な「何と何を並列に進められるか」は各 `stages/stage-N.md` に書き、README に Stage 固有の dependency graph を持ち込まない。

## 4. 最初の milestone 後の roadmap

Stage 8 以降は、実装開始時に `stages/stage-N.md` を作成して具体的な計画を確定します。それまでは roadmap の依存関係だけを保持します。

| Stage | 機能 | 前提 |
|---|---|---|
| 8 | 定期 schedule 実行 | Task・permission・usage 管理 |
| 9 | backup / restore / Host 自動起動 | preservation・管理画面 |
| 10 | Voice | Client UI・permission |
| 11 | Observation | Task・Client UI |
| 12 | group conversation・Companion 間交流 | 会話・Memory |
| 13 | global memory・skill learning・emotion / relationship | Memory・learning |
| 14 | remote Client | device authentication・backup / restore |
| 15 | character edit・package distribution | character management・settings UI |
| 16 | multi-provider・automatic fallback | inference・cost limit |

## 5. 共通 quality gate

正確な command と lint policy は [`AGENTS.md`](../../AGENTS.md) と CI workflow を source of truth とします。実装ガイド側では、各 PR / Stage が満たす性質だけを管理します。

### 各 PR

- 変更した behavior を固定する unit / integration test がある。
- crash、通信断、二重実行、stale premise など、変更に関係する failure path を検証する。
- credential や削除対象本文を log / error / prompt に漏らさない。
- durable state を変更する場合、restart 後の読み戻しまたは recovery を検証する。
- format / lint / test / build の repository CI を通す。

### Stage 完了

- [`acceptance.md`](../requirements/acceptance.md) の該当 scenario を満たす。
- 後続 Stage が依存する public boundary が設計どおり固定されている。
- Windows / Linux の対象経路を両方検証する。
- 未解決 finding は blocker と non-blocking follow-up を分離し、`PROGRESS.md` と Issue の状態を同期する。
