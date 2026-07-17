# ツールを書く

ene が IPC で発見・呼び出しできるツールバイナリを追加します。

## 手順

1. **作成** — 例: ワークスペースで `cargo new --bin tools/<name>`（または git 依存の外部リポジトリ）
2. **アクション定義** — 引数構造体に `#[derive(ToolAction)]` / `ToolSpec` 属性。`async fn run` に本体
3. **提供** — `ene_tool::ActionSetProvider`（または `ene_tool::prelude::*`）でまとめ、手書きのディスパッチループは避ける
4. **サーブ** — `main` で `run_tool_server(Box::new(provider)).await`。常に boxed `dyn ToolProvider`（`run_tool_server::<T>()` はない）
5. **配置** — ene が探す場所（`builtin_tools_dir` / `user_tools_dir`、例: アプリデータの `tools/`）
6. **有効化** — `settings` の `tools.tools` で `"enable": true` と任意の `config`
7. **文書化** — [guide/tools](.)（EN）と `docs/ja/guide/tools/`（JA）
8. **確認** — `cargo run -p ene-cli` → `/tool list`

## 有効化の最小例

```json
{
  "tools": {
    "tools": {
      "my-tool": { "enable": true, "config": {} }
    }
  }
}
```

## さらに深く

- [SDK ガイド](../../reference/tools/sdk.md) — 通し解説とアダプタ
- [Derive マクロ](../../reference/tools/derive-macro.md)
- [ツール IPC 概要](../../reference/tools/overview.md)
- [ツールカタログ](overview.md)
