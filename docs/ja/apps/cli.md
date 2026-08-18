# CLI ユーザーガイド

`ene-ctl` は `ene-core` のターミナルクライアントです。stage / Web と同じ
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
| `--verbose` | `detail` の深さで購読（内面 / thinking） |

`core start` は `ene-core` バイナリを起動し、`api.json` を待って、トークン
**ファイルのパス**（トークンそのものではない）を表示します。`core stop` は
記録した PID にシグナルを送ります。

desktop 専用 API はありません。stage が線上でできることは `ene-ctl` でもできます。
