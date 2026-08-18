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
| `provider.openai_compat` | LLM、埋め込み、TTS、STT（`/v1` chat+audio）。`server_path` を入れると llama-server をループバックで起動します（P-1006）。 |
| `provider.anthropic` | LLM（Messages API） |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS。ユーザー起動エンジン、または `server_path` サイドカー |
| `provider.edge_tts` | TTS（Edge Neural Voice） |

API キーは vault に置き、`settings.json` には書きません。ネイティブの
プロセス内エンジン（llama.cpp、whisper.cpp、Kokoro ONNX）はこのツリーには
ありません。ローカル GGUF 会話は `provider.openai_compat` の
`ai.tasks.*.server_path` に `llama-server` を、`model_path` に GGUF を指定します。
プラグインがそのバイナリをループバックで起動し `/v1` で話します。Sidecar 補助は
`templates/sidecar` にもあります。

MCP の `resources/list` は `<workspace>/mcp-context/` にスナップショットされ、
コンテキスト源として注入されます。`prompts/list` は data-dir の skills 配下の
`SKILL.md` になります。
