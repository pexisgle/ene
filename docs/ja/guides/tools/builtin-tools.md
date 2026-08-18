# 同梱ツール

同梱ツールは `plugins/harness/` にあり、サードパーティと同じ IPC を使います。

| プラグイン | バイナリ | 役割 |
|---|---|---|
| `fs` | `ene-harness-fs` | ワークスペース内の read / write / edit。シェルは持たない |
| `exec` | `ene-harness-exec` | プロセス実行（`fs` から分離、D-24） |
| `web` | `ene-harness-web` | ホスト HTTP 仲介経由の fetch |
| `utility` | `ene-harness-utility` | 決定的な補助（時刻、ハッシュ、エンコード） |

`fs.write` と `exec` は表層スキーマに出ません。レジストリは名前のホワイトリストではなく
空の `side_effects` でフィルタします。承認は deny-by-default で、`ene-plane` に
一致するポリシーがあるまで止まります。

成熟した MCP サーバー（git、browser、calendar、homeassistant、geo）はツリーに
含めません。プロファイル行への手書きで接続します。
