# 同梱ツール

同梱ツールは `plugins/tool/` にあり、サードパーティと同じ IPC を使います。
`ene-core` は起動時に次のプロファイル行を載せます。

| プラグイン | バイナリ | 役割 |
|---|---|---|
| `utility` | `ene-tool-utility` | ハッシュ、時刻、計算、乱数、テキスト |
| `fs` | `ene-tool-fs` | ワークスペース内の read / write / edit / list / search / patch / undo。シェルは持たない |
| `exec` | `ene-tool-exec` | プログラム名でのプロセス実行（`fs` から分離） |
| `web` | `ene-tool-web` | HTTPS fetch と公開検索（SSRF 禁止） |
| `app` | `ene-tool-app` | スクリーンショット、ウィンドウ、クリップボード、ポインタ / キーボード |

`fs.write`、`fs.edit`、`exec`、入力を変える `app.*` は表層スキーマに出ません。
レジストリは名前のホワイトリストではなく空の `side_effects` でフィルタします。
承認は deny-by-default で、`ene-plane` に一致するポリシーがあるまで止まります。
ホスト観測（`app.active_window`、`app.screenshot`）は、ユーザーがプロアクティブ
ソースを有効にしているとき承認ポップアップを飛ばします。

成熟した MCP サーバー（git、browser、calendar、homeassistant、geo）はツリーに
含めません。手書きの `mcp.<id>` 行で接続します。
