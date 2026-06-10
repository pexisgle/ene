# `ene-tool-common`

> Ene ツールバイナリのための `ToolAction` トレイト、共有ヘルパー、および標準プレリュード。

`ene-tool-common` は `tools/` ワークスペース内のあらゆるツールバイナリの主要な依存クレートです。以下を提供します：

- すべてのツールアクションが実装する必要がある **`ToolAction`** および **`ToolSpecArgs`** トレイト。
- 1 行の `use` 文で必要なものをすべて取り込める **`prelude`** モジュール。
- Web コンテンツを扱うツール向けの HTML→Markdown 変換ユーティリティ。
- `ene-common` からのテキスト切り詰めユーティリティの再エクスポート。

関連ページ：トレイトを自動実装するプロシージャルマクロについては [`ene-tool-derive`](ene-tool-derive.md)、基盤となるワイヤー型については [`ene-tool-proto`](ene-tool-proto.md) を参照してください。

---

## `ToolSpecArgs` トレイト

ツールのメタデータを記述する型のための静的ディスパッチインターフェースです。`#[derive(ToolSpec)]` および `#[derive(ToolAction)]` によって自動的に実装されます。

```rust
pub trait ToolSpecArgs {
    /// `namespace.action` 形式の正規ツール名。
    const TOOL_NAME: &'static str;

    /// 人間が読める表示名（`#[tool(display_name = "…")]` で設定）。
    const DISPLAY_NAME: &'static str;

    /// このツールの機能を一文で説明するサマリー。
    const SUMMARY: &'static str;

    /// このアクションの完全な [`ToolSpec`] を構築して返す。
    fn spec() -> ToolSpec;
}
```

これらの定数を直接呼び出す必要はほとんどありません。`ToolAction` の実装と `run_tool_server` によって内部的に使用されます。

---

## `ToolAction` トレイト

すべての実行可能なツールアクションが実装するコアトレイトです。

```rust
pub trait ToolAction: ToolSpecArgs + Send + Sync {
    /// ツールの正規名。`Self::TOOL_NAME` と同じ値。
    fn name(&self) -> &'static str;

    /// このアクションの完全な [`ToolSpec`] を返す。
    fn definition(&self) -> ToolSpec;

    /// JSON エンコードされた引数文字列でアクションを実行する。
    ///
    /// `arguments` は LLM から渡された生の JSON オブジェクトです。
    /// `#[derive(ToolAction)]` で作成された実装では、これは自動的にデシリアライズされます。
    fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

`#[derive(ToolAction)]` を使用した場合、`execute` メソッドはマクロによって生成されます。自分のストラクトに `async fn run(&self) -> Result<String, ToolError>` メソッドを実装するだけで済みます。

---

## `prelude` モジュール

1 行ですべての必要なものをインポートします：

```rust
use ene_tool_common::prelude::*;
```

以下が再エクスポートされます：

| アイテム | ソース |
|---|---|
| `async_trait` | `async-trait` クレート（アトリビュートマクロ） |
| `ToolAction` | このクレート |
| `ToolSpec` | `ene-tool-proto` |
| `tool_action` | `ene-tool-derive` のアトリビュートマクロ |
| `ToolError` | `ene-tool-proto` |
| `JsonSchema` | `schemars` のデライブマクロ |
| `Deserialize` | `serde` のデライブマクロ |

> [!NOTE]
> 他のワークスペースクレートからのすべての再エクスポートには `#[doc(no_inline)]` が付与されており、rustdoc のリンクが元のクレートのドキュメントを参照するようになっています。

---

## `truncate` モジュール

ツールの出力フォーマットに使用するテキスト切り詰めユーティリティを `ene-common` から再エクスポートします。

```rust
pub use ene_common::truncate::{Truncate, TruncateResult};
```

### `Truncate` トレイト

`String` と `&str` に実装されています：

```rust
pub trait Truncate {
    /// 最大 `max_chars` 個の Unicode スカラー値に切り詰める。
    /// 切り詰めが発生したかどうかを示す `TruncateResult` を返す。
    fn truncate_chars(&self, max_chars: usize) -> TruncateResult<'_>;
}
```

### `TruncateResult`

```rust
pub struct TruncateResult<'a> {
    pub content: &'a str,
    /// 元の文字列が max_chars より長かった場合に true。
    pub was_truncated: bool,
}
```

ツールでの典型的な使用例：

```rust
let output = large_text.truncate_chars(8_000);
if output.was_truncated {
    Ok(format!("{}\n\n[出力が切り詰められました]", output.content))
} else {
    Ok(output.content.to_string())
}
```

---

## `html` モジュール

Web コンテンツを取得するツール向けの HTML→Markdown 変換とコンテンツ抽出ユーティリティです。

> [!NOTE]
> このモジュールは [`scraper`](https://crates.io/crates/scraper) クレートをベースとした静的な HTML パースを行います。JavaScript は実行されません。

### 関数

```rust
/// HTML 文字列を Markdown に変換する。
///
/// ナビゲーション、広告、ボイラープレートを除去し、
/// 本文のメインコンテンツを抽出する。
pub fn html_to_markdown(html: &str) -> String;

/// HTML 文字列からプレーンテキストを抽出する（Markdown 書式なし）。
pub fn html_to_text(html: &str) -> String;

/// HTML ドキュメントから <title> を抽出する（存在する場合）。
pub fn extract_title(html: &str) -> Option<String>;
```

### 使用例

```rust
use ene_tool_common::html;

let html = reqwest::get("https://example.com").await?.text().await?;
let title = html::extract_title(&html).unwrap_or_default();
let markdown = html::html_to_markdown(&html);

Ok(format!("# {title}\n\n{markdown}"))
```

---

## ツールバイナリの組み込み方

`ToolAction` を実装したら（手動または derive 経由）、`ene-tool-proto` の `run_tool_server` を使ってサーバーループに組み込みます：

```rust
// tools/my_tool/src/main.rs
use ene_tool_common::prelude::*;
use ene_tool_proto::run_tool_server;

mod actions;
use actions::MyAction;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_tool_server::<MyAction>().await
}
```

`run_tool_server` は、ハンドシェイク・初期化・ツールリスト・ディスパッチループという IPC ライフサイクル全体を処理します。詳細は [`ene-tool-proto`](ene-tool-proto.md) を参照してください。

---

## 関連ページ

- [`ene-tool-derive`](ene-tool-derive.md) — プロシージャルマクロ：`#[derive(ToolAction)]`、`#[derive(ToolSpec)]`
- [`ene-tool-proto`](ene-tool-proto.md) — `ToolSpec`、`ToolError`、`IpcRequest`/`IpcResponse`
- [`ene-tool-host`](ene-tool-host.md) — ホスト側のプロセス管理と `ToolRegistry`
- [ツールの作成方法](../tools/sdk.md)
