# 同梱ツール

同梱ツールは `plugins/tool/` にあり、サードパーティと同じ IPC を使います。
`ene-core` は起動時に次のプロファイル行を載せます。

| プラグイン | バイナリ | 役割 |
|---|---|---|
| `utility` | `ene-tool-utility` | ハッシュ、時刻、system_info、計算（数式・変数・単位・為替スナップショット）、色（hex/rgb/hsl）、乱数（float・整数・pick・UUID v7/v4・色）、テキスト |
| `fs` | `ene-tool-fs` | ワークスペース内の read / write / edit / list / glob / delete / search / patch / undo。シェルは持たない。`fs.glob` はワークスペース相対・件数上限付きで symlink を辿らない。`fs.delete` は承認対象で、ファイルまたは空ディレクトリのみを削除する。search は既定でリテラル、`regex` で正規表現であり、ホストの `rg` に委譲して `include`、大小文字非依存、context、count、capture group、行番号モードを任意指定できる。`fs.read` は `text` と生バイトの blake3 `hash` を返す。`fs.write` / `fs.edit` / `fs.patch` は任意の `expected_hash` を受け付け、不一致時は stale-precondition エラーでファイルを変更しない。書き込みは同一ディレクトリの一時ファイルから rename する原子置換で、同一パスへの操作は直列化される。edit は完全一致を最初に試し、CRLF を正規化した indent 許容・行 trim・block anchor フォールバックを使う。`replace_all` なしで複数一致はあいまいさエラー。改行（CRLF/LF）、UTF-8 BOM、末尾改行を保持する。`fs.undo` は同じジョブ（`job_id` または `ENE_JOB_ID`）が書いたものだけ戻す。秘密らしいパス名と 1 MiB 超の本体は undo ジャーナルに保存しない。unified diff は行番号だけでなく hunk の文脈を照合する。 `tool.fs` 子プロセスにはワークスペース権限を渡さず、fs tool は Fiber の FileBroker で grant 検査してホスト側で実行する。 |
| `exec` | `ene-tool-exec` | プログラム名またはシェルでのプロセス実行（`exec.run` / `exec.shell`）。出力はストリーム読み取り中に上限（stdout 1 MiB、stderr 1 MiB、combined 2 MiB）で打ち切り、打ち切りメタデータを返す。タイムアウト時はプロセスツリー全体にシグナル（Unix はプロセスグループ、Windows は Job Object + `taskkill /T`）。作業ディレクトリは `ENE_WORKSPACE` に閉じ込め、継承 env は許可リストのみで secret 形状の変数は除外する。 |

| `web` | `ene-tool-web` | HTTPS fetch と公開検索。HTTP はホストの net broker が hop ごとに実行する（SSRF、DNS 固定、1 MiB ストリーム上限、テキスト content-type）。プラグインプロセスはネットワーク隔離され、自分では HTTP できない。fetch は markdown/text/html を返す。検索バックエンドは DuckDuckGo（既定）、ArXiv。Tavily/Exa は vault 資格情報が必要。 |
| `app` | `ene-tool-app` | スクリーンショット（Wayland は XDG portal 優先、CLI フォールバック、Windows は GDI）、モニタ、compositor が許す範囲のウィンドウ、native clipboard、入力は X11/Windows のみ |

`fs.write`、`fs.edit`、`fs.delete`、`exec`、入力を変える `app.*` は表層スキーマに出ません。
レジストリは名前のホワイトリストではなく空の `side_effects` でフィルタします。
承認は deny-by-default で、`ene-plane` に一致するポリシーがあるまで止まります。
ホスト観測（`app.active_window`、`app.screenshot`）は、ユーザーがプロアクティブ
ソースを有効にしているとき承認ポップアップを飛ばします。観測経路は `png_base64`
をデコードしてセッション履歴の外で要約します。`{available: false}` は成功した
「見る」ではありません。モデルが `app.screenshot` を呼んだときは PNG を spill
blob に置き、会話ログには `ImageRef` だけを残します。`ai.tasks.chat.supports_images`
が真のときだけ `LlmImage` として渡り、巨大な base64 テキストは積みません。
text-only や能力不明のバインディングは `[image omitted]` のままです。
`harness.tool_output.soft_limit_bytes`（既定 64 KiB）を超えるツール JSON も
同じ spill 経路です。

成熟した MCP サーバー（git、browser、calendar、homeassistant、geo）はツリーに
含めません。手書きの `mcp.<id>` 行で接続します。旧 action の対応、セキュリティ
ギャップ、v1.0 / post-v1.0 は [製品境界](../../concepts/product-boundaries.md)
にあります。
