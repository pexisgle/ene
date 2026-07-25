# `ene-tool-common` — APIリファレンス

> **クレート:** `ene-tool-common`
> **役割:** Eneツールバイナリのための `ToolAction`/`ToolSpecArgs` トレイト、共有ヘルパー、標準プレリュード。

---

## 概要

`ene-tool-common` は `tools/` ワークスペース内のあらゆるツールバイナリの主要な依存クレートです。以下を提供します：

- ツールのメタデータを記述する **`ToolSpecArgs`** トレイト（`#[derive(ToolSpec)]`/`#[derive(ToolAction)]` によって実装される）と、すべてのツールアクションがディスパッチ可能になるために実装する **`ToolAction`** トレイト。
- 1 行の `use` 文で必要なものをすべて取り込める **`prelude`** モジュール。
- Web コンテンツを扱うツール向けの HTML→Markdown 変換とコンテンツ抽出ヘルパー。
- `ene-config` の `Truncate`/`TruncateResult` 構造体 API の再エクスポート。

`ToolAction` は意図的に `ToolSpecArgs` のスーパートレイトに**なっていません** — その理由は [`ToolAction` トレイト](#toolaction-トレイト) を参照してください — これにより、ツールバイナリのディスパッチテーブルは単純な `Vec<Box<dyn ToolAction>>` を保持できます。

関連ページ：これらのトレイトを自動実装するプロシージャルマクロについては [`ene-tool-derive`](./ene-tool-derive.md)、基盤となるワイヤー型（`ToolSpec`、`ToolError`、`ToolProvider`、`run_tool_server`）については [`ene-plugin-proto`](./ene-plugin-proto.md) を参照してください。

---

## `ToolSpecArgs` トレイト

ツールのメタデータを記述する引数ストラクトのための静的ディスパッチインターフェースです。`#[derive(ToolSpec)]` および `#[derive(ToolAction)]` によって自動的に実装されます — 手動での実装は避けてください。

```rust
pub trait ToolSpecArgs: DeserializeOwned + Send + Sync + 'static {
    /// 正規のツール名（例：`"app.press_key"`）。
    const TOOL_NAME: &'static str;

    /// この args 型に対する LLM 向けの `ToolSpec` を返す。
    fn spec() -> ToolSpec;
}
```

| メンバー | 説明 |
|---|---|
| `const TOOL_NAME: &'static str` | このトレイトの唯一の関連定数。 |
| `fn spec() -> ToolSpec` | この args 型に対する完全な `ToolSpec` を構築する。 |

> **注意：** `DISPLAY_NAME` と `SUMMARY` はこのトレイトの**メンバーではありません**。`#[derive(ToolSpec)]` マクロはストラクトに `pub const DISPLAY_NAME: &'static str` と `pub const SUMMARY: &'static str` を生成しますが、これらはトレイトメンバーではなく単なる**固有定数**です。`ToolSpecArgs` の境界を通してではなく、`MyArgs::DISPLAY_NAME` / `MyArgs::SUMMARY` としてアクセスしてください。

`TOOL_NAME`/`spec()` を直接呼び出す必要はほとんどありません。これらは生成された `ToolAction` 実装によって内部的に使用されます。

---

## `ToolAction` トレイト

すべての実行可能なツールアクションが実装するコアトレイトであり、`ToolProvider` 内で動的ディスパッチ（`Box<dyn ToolAction>`）に使用されます。

```rust
#[async_trait]
pub trait ToolAction: Send + Sync {
    /// 正規のツール名を返す。`MyArgs::TOOL_NAME` として実装する。
    fn name(&self) -> &'static str;

    /// このツールのメタデータ定義を返す。`MyArgs::spec()` として実装する。
    fn definition(&self) -> ToolSpec;

    /// JSON 引数文字列でアクションを実行する。
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

### メソッドテーブル

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `name` | `fn name(&self) -> &'static str` | 正規のツール名。実装は `Args::TOOL_NAME` に転送する。 |
| `definition` | `fn definition(&self) -> ToolSpec` | LLM 向けのメタデータ。実装は `Args::spec()` に転送する。 |
| `execute` | `async fn execute(&self, arguments: &str) -> Result<String, ToolError>` | **非同期。** LLM のツール呼び出しに由来する JSON エンコードされた引数文字列でアクションを実行する。 |

`ToolAction` は意図的に `ToolSpecArgs` を拡張していません。（`ToolSpecArgs + Send + Sync` ではなく）単純な `Send + Sync` トレイトのままにすることで、`dyn ToolAction` はオブジェクトセーフのままとなり、単一の `Vec<Box<dyn ToolAction>>` に、異なる無関係な `Args` 型を裏付けとするアクションを保持できます。慣習として、`name()` と `definition()` は args ストラクトの `TOOL_NAME` 定数と `spec()` メソッドへの1行の転送関数であり、これによりスペック名とディスパッチ名が構造的に同じ `&'static str` であることが保証されます。

`ene-tool-derive` の `#[derive(ToolAction)]` マクロは実装全体を生成します — JSON を `Self` にデシリアライズし、`#[tool(skip)]` フィールドをコピーし、`self.run().await` を呼び出す `async` な `execute` を含みます。書くべきなのは `run` の本体だけです。詳しくは [`ene-tool-derive`](./ene-tool-derive.md) を参照してください。

---

## `prelude` モジュール

1 行ですべての必要なものをインポートします：

```rust,no_run
use ene_tool_common::prelude::*;
```

以下が再エクスポートされます：

| アイテム | ソース |
|---|---|
| `async_trait` | `async-trait` クレート（アトリビュートマクロ） |
| `ToolAction`（トレイト、`as _` によって非修飾でスコープに導入される） | このクレート |
| `ToolSpec`、`tool_action`、`ToolSpec`（derive マクロ）、`ToolAction`（derive マクロ） | `ene-tool-derive` |
| `ToolError` | `ene-plugin-proto` |
| `JsonSchema` | `schemars` の derive マクロ |
| `Deserialize` | `serde` の derive マクロ |

> **注意：** 他のワークスペースクレートからのすべての再エクスポートには `#[doc(no_inline)]` が付与されており、rustdoc のリンクが元のクレートのドキュメントを参照するようになっています。

---

## `truncate` モジュール

ツールの出力フォーマットに使用する `Truncate` 構造体 API を `ene-config` から再エクスポートします：

```rust
pub mod truncate {
    pub use ene_config::truncate::{Truncate, TruncateResult};
}
```

`Truncate` は**静的メソッドを持つユニット構造体**です — トレイトではなく、`&str`/`String` に対して直接呼び出せるものはありません。所有は [`ene-config`](./ene-config.md)。メソッドは `Truncate::simple`、`Truncate::detailed`、`Truncate::chars`、`Truncate::output`、`Truncate::tail`、および `TruncateResult`（`content: String`、`truncated: bool`）です。

ツールでの典型的な使用例：

```rust,no_run
use ene_tool_common::truncate::Truncate;

fn format_tool_output(large_text: &str) -> String {
    let output = Truncate::output(large_text, /* max_lines */ 200, /* max_bytes */ 8_000);
    if output.truncated {
        format!("{}\n\n[Output truncated]", output.content)
    } else {
        output.content
    }
}
```

---

## `html` モジュール

Web コンテンツを取得するツール向けの HTML→Markdown 変換とコンテンツ抽出ユーティリティです。

> **注意：** このモジュールは（`htmd` を介して）[`scraper`](https://crates.io/crates/scraper) クレートをベースとした静的な HTML パースを行います。JavaScript は実行されません。

### メソッドテーブル

| 関数 | シグネチャ | 説明 |
|---|---|---|
| `html_to_markdown` | `fn html_to_markdown(html: &str) -> String` | 生の HTML を Markdown に変換する。基盤となるコンバーターが失敗した場合、空文字列ではなく元の HTML をプレーンテキストとして返す。 |
| `extract_html` | `fn extract_html(html: &str, extract: &str, trim: bool) -> String` | ドキュメントの一部（`"body"`、`"main"`、または `"full"`／その他）を抽出し、生の HTML として返す。`trim` が `true` の場合、返す前に非セマンティックなノイズ（`script`、`style`、`nav`、`header`、`footer`、`aside`、`iframe`、`svg` など）を除去する。 |
| `extract_markdown` | `fn extract_markdown(html: &str, extract: &str, trim: bool) -> String` | `extract_html` を適用し（`extract == "full"` かつ `trim == false` の場合を除く。この場合は入力全体をそのまま使用する）、結果を Markdown に変換し、空白を正規化する（連続するスペース／タブを圧縮し、3行以上の改行を1つの空行に圧縮し、トリムする）。 |

> **注意：** このモジュールには `html_to_text` や `extract_title` という関数は**存在しません**。プレーンテキストが必要な場合は、`html_to_markdown`/`extract_markdown` で Markdown に変換し、必要であれば自前で書式を除去してください。タイトルが必要な場合は、生の HTML から自前のセレクタロジック（例：`extract_html` と後続の HTML パーサーの組み合わせ）で抜き出してください。

### 使用例

```rust,no_run
use ene_tool_common::html;

async fn fetch_article(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let raw_html = reqwest::get(url).await?.text().await?;
    let markdown = html::extract_markdown(&raw_html, "main", /* trim */ true);
    Ok(markdown)
}
```

---

## エラー

`ene-tool-common` は独自のエラー型を定義していません。ツール境界を越えるすべての失敗しうる操作は `ene-plugin-proto` の [`ToolError`](./ene-plugin-proto.md#toolerror) を使用します。バリアントの完全な一覧はそのクレートのドキュメントを参照してください。`ToolAction::execute` は `Result<String, ToolError>` を返し、`#[derive(ToolAction)]` によって生成される `execute` は JSON デシリアライズの失敗を `ToolError::InvalidArguments` として報告します。

---

## 使用例

### `ToolAction` を手動で実装する

```rust,no_run
use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolSpec)]
#[tool(
    namespace = "utility",
    name = "echo",
    summary = "Echoes the given text back.",
    category = "Utility",
)]
pub struct EchoArgs {
    /// エコーバックするテキスト。
    pub text: String,
}

pub struct EchoAction;

#[async_trait]
impl ToolAction for EchoAction {
    fn name(&self) -> &'static str {
        EchoArgs::TOOL_NAME
    }

    fn definition(&self) -> ToolSpec {
        EchoArgs::spec()
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: EchoArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;
        Ok(args.text)
    }
}
```

### ツールバイナリの組み込み方

1つ以上の `ToolAction` を実装したら、それらを `ToolProvider` の背後に集約し、そのプロバイダーを `ene-plugin-proto` の `run_tool_server` に渡します。`run_tool_server` は**ジェネリックではありません** — `run_tool_server::<T>()` ではなく、ボックス化されたトレイトオブジェクトを受け取ります。

```rust,no_run
// tools/my_tool/src/main.rs
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec, run_tool_server};

mod actions;
use actions::MyAction;

struct MyToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.actions.iter().map(|a| a.definition()).collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound { tool_name: name.to_string() })
    }

    fn set_session_id(&self, _session_id: &str) {}
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = MyToolProvider { actions: vec![Box::new(MyAction)] };
    run_tool_server(Box::new(provider)).await?;
    Ok(())
}
```

`run_tool_server` は、ハンドシェイク・初期化・ツールリスト・ディスパッチループという IPC ライフサイクル全体を処理します。詳細は [`ene-plugin-proto`](./ene-plugin-proto.md) を参照してください。

---

## 関連ページ

- [`ene-tool-derive`](./ene-tool-derive.md) — プロシージャルマクロ：`#[derive(ToolAction)]`、`#[derive(ToolSpec)]`、`#[tool_action(args = T)]`
- [`ene-plugin-proto`](./ene-plugin-proto.md) — `ToolSpec`、`ToolError`、`ToolProvider`、`run_tool_server`、`IpcRequest`/`IpcResponse`
- [`ene-config`](./ene-config.md) — `Truncate`/`TruncateResult` の所有者
- [`ene-plugin-host`](./ene-plugin-host.md) — ホスト側のプロセス管理と `ToolRegistry`
- [ツールの作成方法](../tools/sdk.md)
