# サンドボックスと承認

ツールが許可されていないものに触れないよう、層は3つです。

1. **OS サンドボックス** (`ene-sandbox`) — Linux では Landlock、seccomp、rlimits。
2. **ホスト仲介** (`ene-fiber`) — spawn、grant、巻き戻し可能な dispose。
   kill は unload ではありません。
3. **承認 plane** (`ene-plane`) — ポリシー行が一致するまで deny-by-default。
   判断は監査ログの hash chain に残ります。

`fs.read` / `fs.write` は親の canonicalization で閉じ込めます（相対 `../` も含む）。
ツールの workspace はデータディレクトリそのものではなく `<data>/workspace` なので、
`api.token` / `vault.key` / `sessions.db` は自動承認の read 対象になりません。

資格情報はボールト (`vault.bin` + `vault.key`) に置き、プラグインの環境変数には
出しません。レジストリが知らないツールは、`side_effects` が空でも機微さ Medium
になります。
