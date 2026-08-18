# 設定

設定の優先順位は defaults → JSON → `ENE_` 環境変数です。
ネストしたキーは `__` で区切ります（例: `ENE_CORE__SERVER__BIND`）。

キーは所有側の `define_config!`（`ene-session`、`ene-kernel`、
`ene-companion`、`ene-body`、`ene-plane` など）に足します。スキーマは起動時に
`assets/schema/` へ再生成されます（gitignored — コミットしない）。

デーモンはデータディレクトリの `settings.json` を読み、その後
`ENE_CORE__SERVER__*` などの環境変数を重ねます。`ene-ctl` と `ene-stage` は
第二の設定スタックを持たず、起動済みコアへは `--url` / `--token`
（または `ENE_API_URL` / `ENE_API_TOKEN`）で接続します。

デバッグビルドの同梱アセットはリポジトリの `assets/` から解決します。
実行時データ（`sessions.db`、`api.token`、`vault.bin`、workspace）は
設定ファイルの隣ではなく、データディレクトリに置きます。
