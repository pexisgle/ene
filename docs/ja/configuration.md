# 設定

設定の優先順位は defaults → JSON → `ENE_` 環境変数です。
ネストしたキーは `__` で区切ります（例: `ENE_CORE__SERVER__BIND`）。

キーは所有側の `define_config!`（`ene-session`、`ene-kernel`、
`ene-companion`、`ene-body`、`ene-plane` など）に足します。スキーマは起動時に
`assets/schema/` へ再生成されます（gitignored — コミットしない）。

デーモンはデータディレクトリの `settings.json` を読み、その後
`ENE_CORE__SERVER__*` などの環境変数を重ねます。`ene-ctl` と `ene-stage` は
`--url` / `--token`（または `ENE_API_URL` / `ENE_API_TOKEN`）で起動済みコアへ
接続します。`ene-desktop` も同じ環境変数があれば接続し、無いときは
`ene-core` を起動します。

データディレクトリは `ENE_DATA_DIR` があればそれです。無ければデバッグビルドは
リポジトリの `assets/` を設定・DB・vault・workspace の根にし、OS のデータ
ディレクトリには書きません。リリースは OS のデータディレクトリだけを使い、
リポジトリの `assets/` は読みません。デスクトップの適用とコアの PATCH は同じ
`settings.json` に書きます。`GET /api/v1/settings` の `effective` はライブ
メモリが正で、ディスクの `overlay` は AI / mind / plugins のライブ値を
上書きしません。API キーは vault のままです。

会話・分類・埋め込み・TTS・STT は `ai.tasks.<task>`（`plugin`、`model`、
`model_path`、`base_url`、`voice`、`max_tokens`）でバインドします。チャットは
未設定のまま起動するので、最初のメッセージの前に `ai.tasks.chat.plugin` を
`provider.*` に設定してください。API キーは vault 秘密です（起動時は
`ENE_AI__TASKS__<TASK>__API_KEY`。PATCH `/api/v1/settings` は JSON に
書きません）。プラグイン id は [プラグイン一覧](concepts/plugins-and-mcp.md) の
`provider.*` です（`GET /api/v1/settings` の `effective.providers`）。
デスクトップは別の許可リストを持ちません。

ローカル GGUF 会話は `provider.gguf`（`local: true`）です。重みと
`llama-server` は AI / Engines の `provider.assets` からインストールします。
任意で `model_path` / `server_path` がカタログを上書きします。クラウド会話は
インストール済みの LLM プラグイン（API キーは vault）です。

埋め込みは任意で、独自の `ai.tasks.embedding` ファイバーです。未設定、
ローカル GGUF（`provider.gguf`、おすすめ Jina）、または `seam.embed` の
クラウドプラグイン。分類・能動発話が未指定なら会話モデルの値を継承します。
TTS・STT が空なら無効のままです。

プラグイン起動は `plugins.profile`（`desktop` / `minimal` / `headless`）です。
プラグインごとの有効マップはありません。

| キー | 役割 |
|---|---|
| `plugins.profile` | 起動ツリー。既定 `desktop`。環境変数: `ENE_PLUGINS__PROFILE`。 |
| `plugins.home_dir` | インストール検索パス。空なら `<data>/plugins`。環境変数: `ENE_PLUGINS__HOME_DIR`。 |
| `plugins.policy.approval_mode` | 起動時に `approval.mode` を初期化（`ask_all` / `policy` / `ai_auto` / `auto`）。実行時の正は `approval.mode`。 |
| `plugins.policy.allow_unverified` | digest 不一致でも起動するか。既定 `false`。 |
| `plugins.ipc.max_frame_bytes` | IPC フレーム上限。既定 `1048576`。環境変数: `ENE_PLUGINS__IPC__MAX_FRAME_BYTES`。 |

MCP サーバーは手書きの `mcp.json` であり、設定キーではありません。
[プラグインと MCP](concepts/plugins-and-mcp.md) を見てください。
