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
`ene-core` を起動します。加えてローカルの `desktop.*`（グラフィックス、
テーマ、言語、マイク、オーバーレイ、コア寿命）はデスクトップ自身の
`settings.json` に保存します。

会話・分類・埋め込み・TTS・STT は `ai.tasks.<task>`（`plugin`、`model`、
`base_url`、`voice`、`max_tokens`）でバインドします。API キーは vault 秘密です
（起動時は `ENE_AI__TASKS__<TASK>__API_KEY`。PATCH `/api/v1/settings` は JSON に
書きません）。プラグイン id は `echo` または [プラグイン一覧](concepts/plugins-and-mcp.md)
の `provider.*` です。

デバッグビルドの同梱アセットはリポジトリの `assets/` から解決します。
実行時データ（`sessions.db`、`api.token`、`vault.bin`、workspace）は
設定ファイルの隣ではなく、データディレクトリに置きます。
