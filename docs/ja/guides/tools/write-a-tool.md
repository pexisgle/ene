# ツールを書く

同梱ツールは `plugins/tool/<name>` のアウトプロセスバイナリです。
`ene-plugin-ipc` の分割 IPC を話し、ローカルの `specs` / `execute` を
`ene_registry::run_tool_plugin` に渡して登録します。

```sh
cargo new --bin plugins/tool/my-tool
```

雛形は `plugins/tool/fs` です。`[[bin]]` のみ、`[lib]` なし、
`ene-plugin-ipc` + `ene-registry`、本体は `src/logic.rs`、`main` から
`ene_registry::run_tool_plugin("my_tool", logic::specs, logic::execute)` を呼びます。
`run_plugin(BuiltinKind::…)` は使わないでください。ホスト側の便宜関数です。
`BuiltinKind` はプラグイン API ではありません。
アクション名は名前空間付き (`my_tool.echo`) にし、`side_effects` を宣言します。
`side_effects` が空であることが、表層レーンに載る条件です。

確認は起動中の `ene-core` に対する `ene-ctl` で行います。同梱プラグインを
足したら [同梱ツール](builtin-tools.md) と日本語版も更新してください。

Rust 以外のフィクスチャは `plugins/tool/dummy-py`（ワークスペース除外）です。
サードパーティツールは同じ core+tool ハンドシェイクを実装し、Ene クレートは
リンクしません。
