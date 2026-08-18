# プラグインと MCP

ツールは **アウトプロセスのバイナリ**です。ホスト (`ene-fiber`) が spawn し、
分割された `core` / `tool` / `provider` 副プロトコル (`ene-plugin-ipc`) を交渉し、
仕様を `ene-registry` に載せます。コンパニオン状態に触るハーネス機能は
ホスト内のまま、同じレジストリパイプラインを通ります。

同梱ツールは `plugins/harness/` にあります: `fs`、`exec`、`web`、`utility`、
`app`。
`exec` は `fs` に含めません。[同梱ツール](../guides/tools/builtin-tools.md) と
[ツールを書く](../guides/tools/write-a-tool.md) を見てください。

MCP サーバーはベンダーしません。手書きの `mcp.json` の各行は `ene-harness-mcp`
（stdio または Streamable HTTP）の `mcp.<id>` ファイバーになり、内製ツールと
同じパイプラインに載ります。コネクターページがそのドキュメントを編集します。
代表的なサーバーを選ぶマーケット UI は後継マイルストーンです。

プロバイダプラグインは `plugins/provider/` にあり、`provider` 副プロトコルを話します。
`ai.tasks.<task>.plugin` でバインドします。

| プラグイン | モダリティ |
|---|---|
| `echo` | ホスト内蔵オフラインモデル（ネットワークなし） |
| `provider.openai_compat` | LLM、埋め込み、TTS、STT（`/v1` chat+audio。llama-server 含む） |
| `provider.anthropic` | LLM（Messages API） |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS（ユーザー起動エンジンへの HTTP） |
| `provider.edge_tts` | TTS（Edge Neural Voice） |

API キーは vault に置き、`settings.json` には書きません。ネイティブの
プロセス内エンジン（llama.cpp、whisper.cpp、Kokoro ONNX）はこのツリーには
ありません。ローカル GGUF 会話は、ユーザー起動の llama-server を
`provider.openai_compat` で指します。Sidecar 補助は `templates/sidecar` にあります。
