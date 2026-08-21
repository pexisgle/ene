# プラグインと MCP

ツールは **アウトプロセスのバイナリ**です。ホスト (`ene-fiber`) が spawn し、
分割された `core` / `tool` / `provider` 副プロトコル (`ene-plugin-ipc`) を交渉し、
仕様を `ene-registry` に載せます。コンパニオン状態に触るハーネス機能は
ホスト内のまま、同じレジストリパイプラインを通ります。

プラグイン監督はカーネルの waterfall（`agent/pre-step`、`agent/request`）を
巻き戻し可能なホスト effect として購読できます。unload は LIFO でリスナーを
外します。サードパーティのツール IPC からは登録できません。

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
| `provider.gguf` | ローカル GGUF の LLM と埋め込み（`plugins/provider/gguf`）。GGUF 重みはプラグインの静的カタログ、`llama-server` はホストの GitHub カタログ（`provider.assets`、Engines ページ）からインストール。任意で `server_path` / `model_path` で上書き。 |
| `provider.openai_compat` | クラウドの LLM、埋め込み、TTS、STT（`/v1` chat+audio）。OpenRouter などは任意の `base_url`。 |
| `provider.anthropic` | LLM（Messages API） |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS。ホスト管理の VOICEVOX Engine（VVPP CPU、`provider.assets`）、またはユーザー起動エンジン / `server_path` |
| `provider.edge_tts` | TTS（Edge Neural Voice） |

API キーは vault に置き、`settings.json` には書きません。ネイティブの
プロセス内エンジン（llama.cpp、whisper.cpp、Kokoro ONNX）はこのツリーには
ありません。ローカル GGUF の会話と埋め込みは `provider.gguf`
（`ene-provider-gguf`）です。プラグインが GGUF 重みの静的カタログを所有し、ホストが
GitHub から `llama-server` リリースを取得し、
`data_dir/plugins/provider.gguf/assets/` に検証済みアーティファクトを置き、
`ene-fiber` 経由で `llama-server` をループバック起動します。Sidecar 補助は
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
デスクトップはベンダ HTTP を呼びません。ローカル GGUF の重みとサイドカーは
汎用の `provider.assets`（`POST /api/v1/providers/assets/*`）で扱います。
`provider.gguf` の一覧は llama-server が既に居るときだけサイドカーの
`/v1/models` です。RPC 未実装の TTS はテキスト入力のままです。

## `provider.assets`

プロバイダプラグインは `assets` フェース（`PROVIDER_ASSETS_VERSION = 1`）を
公開できます。

| メソッド | 役割 |
|---|---|
| `assets.list` | カタログ行とインストール状態 |
| `assets.install` | 非同期インストール開始（`job_id`） |
| `assets.install_status` | 進捗 |
| `assets.set_active` | 使用中バージョンの切替（サイドカー） |

種別は拡張可能な文字列（`sidecar`、`weight` など）。`assets` を交渉した
プラグインはデスクトップが同じ UI を描画し、`ene-core` が HTTP で中継します
（`POST /api/v1/providers/assets/*`、`refresh_catalog` で手動更新）。

**ホスト管理カタログ。** `provider.gguf` の `llama-server` と
`provider.voicevox` の `voicevox-engine` は、プラグインの静的ソースではなく
ホスト（`ene-fiber` + `ene-provider-assets`）が一覧・インストールします。起動時
（および手動更新時）に `ggml-org/llama.cpp` と `VOICEVOX/voicevox_engine` の
GitHub Releases を取得し、`data_dir/catalog-cache/` に JSON をキャッシュ、各
プラグインの `manifest.json` とマージします。インストールキーは
`{release_tag}/{variant_id}`（例: `b4282/avx2`、`0.25.2/cpu`、`0.25.2/directml`）。Engines UI で
リリースとバックエンドバリアントを選んでからダウンロードします。VOICEVOX の
CUDA/NVIDIA 向け `.vvppp` / `.7z.001` 分割パッケージは現状ホストでは未対応です。

**プラグイン所有カタログ。** GGUF 重みは `provider.gguf` 内の静的 Hugging Face
URL のままです。重みはプラグイン probe の `assets.list`、サイドカー行はホストが
上書きします。

**ダウンロード。** ホストが GitHub（重みは Hugging Face）の固定プレフィックスで
URL を検証し、ディスクへストリーミング、GitHub が digest を返せば SHA-256 検証、
初回インストール後はローカル digest を記録します。VVPP CPU は zip ツリー全体を展開、
llama-server は zip 全体を展開（Windows では `ggml.dll` 等が実行ファイルと同じ
ディレクトリに必要）。CUDA は `cudart-*` コンパニオン zip も同ディレクトリに展開します。

**サイドカー注入。** インストール後、設定に未設定なら `ene-fiber` が
`provider.gguf` に `sidecar_base_url`、`provider.voicevox` に `cas_path` を注入します。
