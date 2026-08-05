# ツールプラグインを書く

このガイドでは、リポジトリのテンプレートから新しいツールプラグインを
作成します。ツールプラグインは、名前付きアクションをキャラクターに公開する
小さな Rust バイナリです。

## 1. 雛形を作る

```sh
templates/tool/new-tool.sh my_tool
```

これで `plugins/tool/my_tool/` が作成されます:

- クレート/バイナリ名 `ene-plugin-my_tool`、
- ツール名前空間 `my_tool`（第 2 引数で変更可）、
- `my_tool.echo` アクションを 1 つ持つプロバイダー構造体、
- ワークスペースメンバー登録（workspace の `members` glob が拾います）。

## 2. ツールプラグインの構造

```text
plugins/tool/my_tool/
├── Cargo.toml        # bin クレート。deps: ene-plugin, serde, schemars, tokio
└── src/
    ├── main.rs       # run_plugin_server(PluginDispatch::new(...))
    ├── provider.rs   # ActionSetProvider: アクション一覧 + ライフサイクル
    └── action.rs     # アクションごとに 1 構造体。ToolAction を derive
```

アクションはプレーンな構造体です。フィールドが JSON 引数になり、
`#[tool(...)]` 属性がスキーマメタデータを宣言し、`run` メソッドが挙動を
実装します:

```rust
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "my_tool",
    name = "echo",
    summary = "Echo text back.",
    description = "Returns the input text unchanged. Use for testing.",
    category = "Utility"
)]
pub struct EchoAction {
    /// Echo するテキスト。
    #[arg(min_length = 1, max_length = 2000)]
    text: String,
}

impl EchoAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(self.text.clone())
    }
}
```

プロバイダーがアクションを登録し、`run_plugin_server` が配信します:

```rust
use ene_plugin::{ActionSetProvider, PluginDispatch, run_plugin_server};

struct MyToolProvider;

impl ActionSetProvider for MyToolProvider {
    fn actions(&self) -> Vec<Box<dyn ToolAction>> {
        vec![Box::new(EchoAction::default())]
    }
}

#[tokio::main]
async fn main() -> Result<(), PluginError> {
    run_plugin_server(PluginDispatch::new(
        Some(Arc::new(MyToolProvider)),
        None, None, None, None,
    ))
    .await
}
```

## 3. 副作用とバックグラウンド処理の宣言

ホストはアクションが何に触れるかを知る必要があります:

```rust
#[tool(
    namespace = "fs",
    name = "write",
    side_effects = "FileSystem { mutates: true }"
)]
```

副作用のあるアクションは権限ゲートを通ります（ユーザーが一度だけ・
セッション中・永続のいずれかで承認）。長時間実行するアクションは
`background_capable` を宣言できます。ホストはターンをブロックせずに
遅延タスクとして実行し、完了をライフサイクルバスで通知します。

属性の完全な一覧は[ツール SDK リファレンス](../../reference/tools/sdk.md)を
参照してください。

## 4. 状態を持つツール

永続状態が必要な場合は、`ene-plugin-db` を通じて `db` ホストサービスを
使います。プラグインはホストの `memory.db` 内でトークン認証・プレフィックス
分離された CRUD を実行できます — ローカルファイルも独自 DB も不要です。
`plugins/tool/counter` サンプルが参照実装です（状態・権限ゲート・DB IPC・
統合テスト）。

## 5. 登録して確認

```json
{
  "plugins": {
    "list": {
      "my_tool": { "enable": true }
    }
  }
}
```

```sh
cargo build -p ene-plugin-my_tool
cargo run -p ene-cli -- tool list          # my_tool.echo が表示される
# REPL で:
/tool call my_tool.echo '{"text": "hi"}'
```

## 6. リポジトリのルール

- 名前は名前空間付き: `<namespace>.<action>`。
- プラグインクレートは**バイナリのみ**（`[lib]` ターゲットなし）。
  統合テストや他クレートがロジックをリンクする必要がある場合だけ lib を
  追加します。
- ドキュメントを同期: リポジトリ同梱のツールは
  [同梱ツール](builtin-tools.md) に載せ、`docs/` / `docs/ja/` の二言語
  ページを更新します。
- リントが仕様です。push 前に
  `cargo clippy --workspace --all-targets -- -D warnings` を実行してください。
