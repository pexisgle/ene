# プラグインと MCP

ツールは **アウトプロセスのバイナリ**です。ホスト (`ene-fiber`) が spawn し、
分割された `core` / `tool` / `provider` / `capability` 副プロトコル (`ene-plugin-ipc`) を交渉し、
仕様を `ene-registry` に載せます。コンパニオン状態に触るハーネス機能は
ホスト内のまま、同じレジストリパイプラインを通ります。

プラグイン監督はカーネルの waterfall（`agent/pre-step`、`agent/request`）を
巻き戻し可能なホスト effect として購読できます。unload は LIFO でリスナーを
外します。サードパーティのツール IPC からは登録できません。

同梱ツールは `plugins/tool/` にあります: `fs`、`exec`、`web`、`utility`、
`app`。各バイナリが自分の `specs` / `execute` を持ち、サードパーティと同じ
tool IPC を話します。
`exec` は `fs` に含めません。`web.fetch` / `web.search` はホストの net broker
（`net.fetch` grant）上で動き、`tool.web` プロセスはネットワーク隔離されて
ソケットを開きません。[同梱ツール](../guides/tools/builtin-tools.md) と
[ツールを書く](../guides/tools/write-a-tool.md) を見てください。

仕様には任意の discovery メタデータ（`category`、`keywords`、`examples`）を
載せられます。`ene-registry` は登録されたすべてのツール（同梱プラグイン、MCP、
ハーネス）を同じ経路で索引し、埋め込みなしの字句検索 `search_tools(query, limit)`
をホスト向けに提供します。プラグイン unload で該当行は索引から外れます。

MCP サーバーはベンダーしません。手書きの `mcp.json` の各行は `ene-tool-mcp`
（stdio または Streamable HTTP）の `mcp.<id>` ファイバーになり、内製ツールと
同じパイプラインに載ります。プロセス受入は実 `git` を呼ぶ stdio サーバー
（`mcp:git.status` / `mcp:git.log`）です。stage の **Connections** がその
ドキュメントを編集します（旧 desktop の Connectors も同じ）。代表的な
サーバーを選ぶマーケット UI は stage 上の後継
（[#812](https://github.com/pexisgle/ene/issues/812)、P-616）です。

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
| `provider.gguf` | ローカル GGUF の LLM と埋め込み（`plugins/provider/gguf`）。GGUF 重みはプラグインの静的カタログ、`llama-server` はホストの GitHub カタログ（`provider.assets`、Engines ページ）からインストール。任意で `server_path` / `model_path` で上書き。チャット補完は SSE トークンを `LlmChunk` に載せる。 |
| `provider.openai_compat` | クラウドの LLM、埋め込み、TTS、STT（`/v1` chat+audio）。OpenRouter などは任意の `base_url`。チャット補完は SSE トークンを `LlmChunk` に載せ、ストリーム失敗時は一括生成へフォールバックする。 |
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

`hello_ack` で `capability` を名乗るプラグインは Broker RPC
（`capability.request`、`capability.approval_query`）を呼び、grant を受け取れます。
`plugins.ipc.bulk_threshold_bytes` を超える本体は MessagePack フレームに載せません。
ホストが `stream.open` でバルク流を開き、Linux では `SCM_RIGHTS` でソケットを渡します。
`capability` を省いたツールプラグインは `core` + `tool` のまま動きます。

MCP の `resources/list` は `<workspace>/mcp-context/` にスナップショットされ、
`mcp.resources` コンテキスト源として注入されます。`prompts/list` は data-dir の
skills 配下の `SKILL.md` になります。対話レーンはそのカタログを
`skills.catalog` として載せ、依頼に合う本文を `skills.active` として注入します。
`ene.proactive_hint` / `ene.emotion_note` の frontmatter は能動発話判定と
感情分類へ渡ります。soul ごとに許可リストがあり
（`PATCH /api/v1/souls/{id}/skills`。空は導入済みすべて）、
ツールは `skill.list` で一覧を発見し、返された正確な ID を `skill.load` に渡します。
未知の名前は `unknown_skill` になります。
ジョブ層の `workflow.bookmark` はテーマを調べて Markdown を交付します。

## Background tools

ツール仕様は `background: true` を付けられます（ホスト専用。モデル schema には
出ません）。ホストが安定した `execution_id` を割り当て、`ene-work` のジョブ
（`companions.db` の `tool_executions`。第二の task store ではない）に永続し、
`{execution_id, status: "started"}` を返して会話ターンを解放します。

プラグインは `tool_background_start` / `cancel` / `status` を受けます（cancel と
status は冪等、未知 id は `unknown`）。完了は `tool_execution_complete`
通知です。取りこぼしは status 監視で補い、job/report レーンへ **一度だけ**
届きます（`completion_delivered`）。

| プラグイン / ホスト事象 | 永続 status | report intent |
|---|---|---|
| 成功 | `completed` | `tool_complete` |
| キャンセル | `cancelled` | `tool_cancelled` |
| ホスト期限 | `timed_out` | `tool_timeout` |
| プラグインクラッシュ / IPC切断 | `plugin_crash` | `tool_plugin_crash` |
| 実行中のホスト再起動 | `plugin_crash`（`error_class=host_restart`） | `tool_plugin_crash` |

cancel 時にプラグインは子プロセスを止めます。`background` のない同期
`tool_call` / `tool_result` と MCP ツールは従来どおりです。

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

## プラグイン設定

プラグインは JSON Schema の宣言、候補値の validation、dynamic options、
適用を tool IPC で出せます。`HelloAck.has_config` の既定は false なので、
設定を持たないプラグインは追加実装なしで動きます。ホストはそれらの
プラグインへ config RPC を送りません。

秘密フィールド（`x-ene-secret`、`writeOnly`、`format: password`）は
schema 上の名前だけです。値は vault 側にあり、API / UI / ログには
redact したオブジェクトだけが出ます。`GET /api/v1/plugins/{id}/config` が
schema と redact 済みの現在値です。検証は `POST .../config/validate`、
適用は `PUT .../config`。dynamic options（`POST .../config/options`）は
列挙に失敗すると手入力へ縮退します（`fallback: true`）。適用失敗時は
直前の有効な `ProfileRow.config` を保持します。適用が成功すると、秘密ではない
値は `plugin-config.json`（row id キー）へ、秘密フィールドは vault の
`plugin.config.{row_id}.{field}` へ残します。`apply_plugin_profile` が
収集した行へこれらを重ねるので、デーモン再起動後も設定が残ります。Stage の
Connections は同じ文書を選択中のファイバーへ出します。

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
