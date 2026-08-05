# CLI ユーザーガイド

`ene-cli` はターミナルクライアントです。キャラクターとチャットする対話型
REPL と、スクリプトや CI 向けの非対話サブコマンドを提供します。

```sh
cargo run -p ene-cli -- <フラグ> [サブコマンド]
```

## グローバルフラグ

| フラグ | 意味 |
|---|---|
| `--config <path>` | 別の `settings.json` を読み込む |
| `--character <name>` | 設定済みキャラクターを上書き |
| `--lang <en\|ja>` | UI 言語を上書き |

サブコマンドなしで起動すると REPL が始まります。

## REPL

メッセージを打つとキャラクターと会話できます。スラッシュコマンド:

| コマンド | 使い方 | 目的 |
|---|---|---|
| `/help` | `/help` | 全コマンドを一覧表示 |
| `/quit`, `/exit` | | REPL を終了 |
| `/clear` | `/clear` | 画面をクリア |
| `/affect` | `/affect <show\|reset>` | キャラクターの PAD 感情状態を確認/リセット |
| `/prompt` | `/prompt` | 直近ターンの構成済みプロンプトパケットを表示（デバッグ） |
| `/card` | `/card <name>` | キャラクターカードを切替 |
| `/characters` | `/characters` | 発見されたキャラクターを一覧表示 |
| `/import` | `/import <path>` | PNG/CHARX キャラクターカードをインポート |
| `/config` | `/config [set <ドット区切りキー> <値>]` | 実行時設定の表示/変更 |
| `/history` | `/history` | 会話履歴を表示 |
| `/undo` | `/undo` | 直前の状態変更操作を取り消す |
| `/tool` | `/tool <list\|search\|help\|call>` | ツールの確認と直接呼び出し |
| `/memory` | `/memory <list\|inspect\|search\|why\|pin\|archive\|forget\|dispute\|restore\|status\|pending\|retry\|approval>` | 型付きメモリの管理 |
| `/commitments` | `/commitments <list\|done <id>>` | 約束台帳の管理 |
| `/session` | `/session <info\|split\|summaries\|list\|export\|import\|search\|archive\|unarchive>` | セッション管理 |
| `/permissions` | `/permissions <list\|revoke\|reset>` | 付与済み権限の管理 |
| `/connector` | `/connector <list\|status\|check\|connect\|disconnect\|grant\|revoke\|permissions>` | 外部サービスコネクタの管理 |
| `/schedule` | `/schedule <list\|add\|history\|delete\|pause\|resume>` | 永続スケジュールの管理 |
| `/doctor` | `/doctor` | 環境ヘルスチェック |
| `/greeting` | `/greeting [<index>\|none]` | 挨拶メッセージの切替 |
| `/store` | `/store <backup\|list-backups\|restore\|integrity>` | DB のバックアップ/復元/整合性 |
| `/workspace` | `/workspace <sync\|cancel\|status\|search <query>>` | ワークスペース RAG インデックス管理 |

## 非対話サブコマンド

1 回の操作を実行して終了します。多くは機械可読出力の `--json` を受け付けます。

### `ene run`

プロンプトを 1 回実行して応答をストリーミングし、終了します:

```sh
ene run "今日の天気は？"
ene run --jsonl "物語を聞かせて"       # 1 行に 1 つの JSON イベント
ene run --json "こんにちは"            # 単一の JSON サマリー
ene run --timeout 60 --yes "消して /tmp/scratch"  # ツールを自動承認、時間制限付き
```

`--yes` は副作用のあるツール操作を自動承認します（信頼できるスクリプト環境
向け）。これがない場合、権限ゲート付きツールはプロンプトせずに実行を失敗
させます。プロンプト引数がない場合は stdin から読みます。

`--jsonl` ストリームは API v1 イベントスキーマ
（[`PublicChatEvent`](../reference/architecture/api-v1.md)）を使います。

### `ene tool`

```sh
ene tool list
ene tool search "calendar"
ene tool help fs.write
ene tool call fs.read '{"path": "Cargo.toml"}'
```

### `ene session`

```sh
ene session list
ene session list --archived
ene session export <id>        # バージョン付き・リダクション済み JSON バンドル
ene session import <path>
ene session search "クエリ"
ene session archive <id>
```

### `ene characters`

```sh
ene characters list                # 名前・カードパス・アセット
ene characters import <card.png|card.charx>
```

### `ene memory`

型付きメモリストアの照会:

```sh
ene memory list [--kind <KIND>]
ene memory inspect <id>
ene memory search "キャンプ"
```

ピン留め・アーカイブ・忘却・異議・復元・承認キューなどの完全な管理は
REPL の `/memory` で行えます。

### `ene doctor`

環境ヘルスチェック: 設定の妥当性・プロバイダー到達性・ストア整合性・
プラグイン状態。終了コードに結果が反映されるため CI に使えます。

### `ene store`

```sh
ene store backup
ene store list-backups
ene store restore <path> --yes      # --yes で破壊的な復元を確認
ene store integrity
```

## 例

```sh
# 別キャラクターと非対話でチャット
ene --character "Mira" run "おはよう"

# スクリプト: 最新セッションをファイルにエクスポート
ene session list --json | jq '.[0].id' | xargs ene session export

# cron ジョブのヘルスゲート
ene doctor --json
```

## ローカライズ

CLI の UI はローカライズされています（`en-US`・`ja`）。`--lang` でシステム
ロケールを上書きできます。スラッシュコマンド名と JSONL イベントスキーマは
英語のままです。
