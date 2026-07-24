# ツールを書く

ene が IPC で発見・呼び出しできるツールプラグインバイナリを追加します。

## 手順

1. **作成** — 例: ワークスペースで `cargo new --bin plugins/tool/<name>`（または git 依存の外部リポジトリ）
2. **アクション定義** — 引数構造体に `#[derive(ToolAction)]` と `#[tool(...)]` 属性を付け、`async fn run` に処理本体を実装
3. **提供** — `ene_tool_common::ActionSetProvider`（または `ene_tool_common::prelude::*`）でまとめ、手書きのディスパッチループは避ける
4. **ラップしてサーブ** — プロバイダを `ene_plugin::ToolPluginAdapter` でラップし、`main` で `run_plugin_server(Box::new(ToolPluginAdapter(provider))).await`
5. **インストール** — ene が探索する場所（`builtin_plugins_dir` / `user_plugins_dir`、例: アプリデータの `plugins/`）にバイナリを配置。バイナリは `ene-plugin-{name}` の命名規則に従う
6. **有効化** — `settings` の `plugins.list` で `"enable": true` とフラット化された任意設定
7. **文書化** — [guide/tools](.)（EN）と `docs/ja/guide/tools/`（JA）
8. **確認** — `cargo run -p ene-cli` → `/tool list`

## 有効化の最小例

```json
{
  "plugins": {
    "list": {
      "my-tool": { "enable": true }
    }
  }
}
```

## さらに深く

- [SDK ガイド](../../reference/tools/sdk.md) — 通し解説とアダプタ
- [Derive マクロ](../../reference/tools/derive-macro.md)
- [ツール IPC 概要](../../reference/tools/overview.md)
- [ツールカタログ](overview.md)
