# プラグインと MCP

ツールは **アウトプロセスのバイナリ**です。ホスト (`ene-fiber`) が spawn し、
分割された `core` / `tool` 副プロトコル (`ene-plugin-ipc`) を交渉し、
仕様を `ene-registry` に載せます。コンパニオン状態に触るハーネス機能は
ホスト内のまま、同じレジストリパイプラインを通ります。

同梱ツールは `plugins/harness/` にあります: `fs`、`exec`、`web`、`utility`。
`exec` は `fs` に含めません。[同梱ツール](../guides/tools/builtin-tools.md) と
[ツールを書く](../guides/tools/write-a-tool.md) を見てください。

MCP サーバーはベンダーしません。v1.0 はプロファイル行への手書きで、
内製ツールと同じパイプラインに載せます。代表的なサーバーを選ぶ設定 UI は
後継マイルストーンです。

プロバイダプラグイン（LLM / TTS / STT）はこのツリーにはまだありません。
新しい IPC へ書き直すまで会話は Echo のみです。将来の書き換え用 sidecar
補助は `templates/sidecar` にあります。
