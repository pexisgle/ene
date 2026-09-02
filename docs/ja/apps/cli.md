# CLI ユーザーガイド

`ene-ctl` は `ene-core` のターミナルクライアントです。stage / Web / 旧 desktop と同じ
公開 HTTP/WS API を使います。

```sh
cargo run -p ene-ctl -- --help
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- core status
```

| フラグ / 環境変数 | 意味 |
|---|---|
| `--url` / `ENE_API_URL` | `ene-core` のベース URL |
| `--token` / `ENE_API_TOKEN` | Bearer トークン（または `api.token` の中身） |
| `--client-id` | 排他資源用のクライアント ID（既定 `cli`） |
| `--verbose` | `detail` の深さで購読（内面 / thinking / ツール引数） |

`core start` は `ene-core` バイナリを起動し、`api.json` を待って、トークン
**ファイルのパス**（トークンそのものではない）を表示します。`core stop` は
記録した PID にシグナルを送ります。

`ene-ctl task list` / `task cancel` / `task answer <id> <text>` がジョブを
扱います。`task answer` は `POST /api/v1/jobs/{id}/answer` で、チャットの
follow-up ではありません。

desktop 専用 API はありません。stage が線上でできることは `ene-ctl` でもできます。

通常の対話出力は**表層**のままです。内面・thinking・ツール引数は `--verbose` か
`ene-ctl debug` のときだけ出ます。

| コマンド | 意味 |
|---|---|
| `soul list` / `soul show <id>` | soul 一覧 / 詳細 |
| `soul skills <id> [names…]` | soul の skill 許可リストを置き換える（`PATCH /souls/{id}/skills`）。名前を省略すると導入済みすべてが対象 |
| `chat <target> [text]` | `text` があれば一回送信。省略すると REPL（`.quit` / `.exit` / EOF）。`target` はセッション ID、または開いている会話を再利用／新規作成する soul ID |
| `session list/show/create/fork/export/compact` | セッションのライフサイクル |
| `session search <query>` / `split <id>` / `end <id>` | 検索、現在ターンで分割、明示終了 |
| `usage [--session <id>]` | LLM 使用量 |
| `tool list` / `tool call <name> [json]` | ツール一覧、または直接実行（`POST /tools/{name}/test`）。`json` の既定は `{}` |
| `plugin list/restart/config` | プラグイン fiber |
| `task list/cancel/answer` | ジョブ（公開委譲） |
| `memory list/edit/delete` | 記憶行 |
| `schedule list` / `schedule add <soul> <name> <spec>` | スケジュール一覧、または作成。`spec` は 5 か 6 フィールドの cron（空白を含むなら引用）。`--timezone`（既定 `UTC`）、`--action`（`remind` / `job` / `turn`）、`--action-ref`、`--important` |
| `core start/status/stop` | コア |
| `debug log <session>` | detail 深度の履歴 |
| `debug delegation <id>` | ジョブ ID または委譲セッション ID を detail 深度で表示 |
| `debug spans` | 診断スパン |
| `exclusive show/claim` | 排他資源 |

```sh
ene-ctl chat <soul-id>
ene-ctl chat <session-id> "hello"
ene-ctl schedule add <soul-id> morning "0 9 * * *" --timezone Asia/Tokyo
ene-ctl tool call utility.time
ene-ctl debug delegation <job-id>
```
