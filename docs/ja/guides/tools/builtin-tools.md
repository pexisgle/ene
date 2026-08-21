# 同梱ツール

同梱ツールは `plugins/tool/` にあり、サードパーティと同じ IPC を使います。
`ene-core` は起動時に次のプロファイル行を載せます。

| プラグイン | バイナリ | 役割 |
|---|---|---|
| `utility` | `ene-tool-utility` | ハッシュ、時刻、system_info、計算（単位と為替テーブル）、乱数、テキスト |
| `fs` | `ene-tool-fs` | ワークスペース内の read / write / edit / list / search / patch / undo。シェルは持たない。search は既定でリテラル、`regex` で正規表現。`fs.undo` は同じジョブ（`job_id` または `ENE_JOB_ID`）が書いたものだけ戻す。unified diff は行番号だけでなく hunk の文脈を照合する。 |
| `exec` | `ene-tool-exec` | プログラム名でのプロセス実行（`fs` から分離）。タイムアウトは SIGTERM のあと SIGKILL。終了すればキャプチャした出力を返す。 |
| `web` | `ene-tool-web` | HTTPS fetch（サイズ上限、SSRF 禁止）と公開検索（DuckDuckGo Instant Answer、空なら HTML フォールバック） |
| `app` | `ene-tool-app` | スクリーンショット（Wayland は XDG portal 優先、CLI フォールバック、Windows は GDI）、モニタ、compositor が許す範囲のウィンドウ、native clipboard、入力は X11/Windows のみ |

`fs.write`、`fs.edit`、`exec`、入力を変える `app.*` は表層スキーマに出ません。
レジストリは名前のホワイトリストではなく空の `side_effects` でフィルタします。
承認は deny-by-default で、`ene-plane` に一致するポリシーがあるまで止まります。
ホスト観測（`app.active_window`、`app.screenshot`）は、ユーザーがプロアクティブ
ソースを有効にしているとき承認ポップアップを飛ばします。観測経路は `png_base64`
をデコードしてセッション履歴の外で要約します。`{available: false}` は成功した
「見る」ではありません。

成熟した MCP サーバー（git、browser、calendar、homeassistant、geo）はツリーに
含めません。手書きの `mcp.<id>` 行で接続します。旧 action の対応、セキュリティ
ギャップ、v1.0 / post-v1.0 は [製品境界](../../concepts/product-boundaries.md)
にあります。
