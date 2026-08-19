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
ホストカタログ（`ene_fiber::PROVIDER_PLUGINS`）が唯一の一覧です。デスクトップの
選択、Engines、`ai.tasks.*` はすべてそれ（`effective.providers`）を読みます。
プロバイダを足すのは、バイナリと seams / `local` / `needs_key` 付きのカタログ行を
足すことであり、UI 側の第二の許可リストではありません。

`ai.tasks.<task>.plugin` でカタログ id を結びます。設定したタスクごとに
ファイバー（`row_id = ai.tasks.<task>`）が立つので、会話と埋め込みが同じ
バイナリでも別 GGUF を持てます。

| プラグイン | モダリティ |
|---|---|
| `provider.gguf` | ローカル GGUF の LLM と埋め込み（`plugins/provider/gguf`）。`model_path` を指定。`server_path` が空なら `PATH` または同梱から `llama-server` を解決。 |
| `provider.openai_compat` | クラウドの LLM、埋め込み、TTS、STT（`/v1` chat+audio）。OpenRouter などは任意の `base_url`。 |
| `provider.anthropic` | LLM（Messages API） |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS。ユーザー起動エンジン、または `server_path` サイドカー |
| `provider.edge_tts` | TTS（Edge Neural Voice） |

API キーは vault に置き、`settings.json` には書きません。ネイティブの
プロセス内エンジン（llama.cpp、whisper.cpp、Kokoro ONNX）はこのツリーには
ありません。ローカル GGUF の会話と埋め込みは `provider.gguf`
（`ene-provider-gguf`）に `model_path` を付けます。タスクごとのサイドカーが
`llama-server` をループバックで起動し `/v1` で話します。Sidecar 補助は
`templates/sidecar` にもあります。

MCP の `resources/list` は `<workspace>/mcp-context/` にスナップショットされ、
コンテキスト源として注入されます。`prompts/list` は data-dir の skills 配下の
`SKILL.md` になります。

## 起動プロファイル

`plugins.profile` がハーネスの起動ツリーを選びます。`apply_profile` がファイバーを
差分 reconcile し、無関係な行は動かしません。

| プロファイル | ハーネスプラグイン | MCP |
|---|---|---|
| `desktop`（既定） | `tool.utility`、`tool.fs`、`tool.exec`、`tool.web`、`tool.app` | 手書き `mcp.json` |
| `minimal` | `tool.utility` | なし |
| `headless` | `tool.utility`、`tool.fs`、`tool.exec`、`tool.web` | 手書き `mcp.json` |

プロバイダはホストカタログから来て、`ai.tasks.*` に結んだとき起動します。
プロファイル名ではありません。プロファイルの変更は
プラグインページ、または `PATCH /api/v1/settings` の
`{"plugins":{"profile":"minimal"}}` です。

リモートの在庫（OpenAI 互換 `/models`、Anthropic `v1/models`）はプロバイダ RPC
（`list_models`）です。コアは `POST /api/v1/providers/models` で出します
（plugin、task、下書きの base URL、入力中のキー。空なら vault）。
デスクトップはベンダ HTTP を呼びません。ローカル GGUF ファイルはホストの
カタログとファイル選択のままです。プラグインは重みをダウンロードしません。
`provider.gguf` の一覧は llama-server が既に居るときだけサイドカーの
`/v1/models` です。RPC 未実装の TTS はテキスト入力のままです。
