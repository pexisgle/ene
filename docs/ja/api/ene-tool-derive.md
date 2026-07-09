# `ene-tool-derive`

> `ToolSpec` メタデータと `ToolAction` 実装をアノテーション付きストラクトから生成するプロシージャルマクロ。

`ene-tool-derive` は Ene ツールを作成する際のボイラープレートを排除する 3 つのマクロを提供します：

| マクロ | 種別 | 目的 |
|---|---|---|
| `#[derive(ToolSpec)]` | Derive | `impl ToolSpecArgs` （メタデータのみ）を生成 |
| `#[derive(ToolAction)]` | Derive | `impl ToolSpecArgs` **+** `execute()` を含む `impl ToolAction` を生成 |
| `#[tool_action(args = T)]` | アトリビュート | 手動 `impl ToolAction` ブロックに `name()` と `definition()` を補完 |

大部分のケースでは `#[derive(ToolAction)]` を使用してください。これは `#[derive(ToolSpec)]` を含んでおり、完全な実装を生成します。

---

## `#[derive(ToolSpec)]`

ストラクトに直接（**固有アイテム**として、単純な `impl MyArgs { ... }` ブロック内に）以下を生成します：
- `pub const TOOL_NAME: &'static str`
- `pub const DISPLAY_NAME: &'static str`
- `pub const SUMMARY: &'static str`
- `pub fn spec() -> ToolSpec`

...そして別に、`TOOL_NAME` と `spec()` のみを持つ `impl ToolSpecArgs for MyArgs` を生成します：

```rust
impl ToolSpecArgs for MyArgs {
    const TOOL_NAME: &'static str = /* ... */;
    fn spec() -> ToolSpec {
        Self::spec() // 上記の固有関数に委譲する
    }
}
```

> **重要：** `DISPLAY_NAME` と `SUMMARY` は**ストラクトのみに存在する固有定数**です — `ToolSpecArgs` トレイトの一部では*ありません*（[`ene-tool-common`](./ene-tool-common.md#toolspecargs-トレイト) を参照）。常に具体的な型に対して直接 `MyArgs::DISPLAY_NAME` / `MyArgs::SUMMARY` としてアクセスしてください。汎用的な `T: ToolSpecArgs` 境界を通してはアクセスできません。

`parameters` の JSON スキーマは `schemars` を使用してストラクトのフィールドから生成されます。マクロはルートスキーマオブジェクトに自動的に `additionalProperties: false` を設定します。

### コンテナアトリビュート — `#[tool(...)]`

**ストラクト自体**に配置します：

| アトリビュート | 型 | 必須 | 説明 |
|---|---|---|---|
| `namespace` | `&str` | Yes | ツール名の名前空間プレフィックス（例：`"fs"`）。 |
| `name` | `&str` | Yes | 名前空間内のアクション名（例：`"read_file"`）。完全名は `namespace.name` になります。 |
| `display_name` | `&str` | No | 人間が読める表示名。省略時は `name` をタイトルケースにしたもの。 |
| `summary` | `&str` | Yes | LLM のツールリスト向けの一文説明。 |
| `description` | `&str` | No | ツール詳細表示向けの長い説明。省略時は `summary` と同じ。 |
| `category` | `&str` | No | カテゴリ文字列（例：`"Filesystem"`、`"Web"`）。 |
| `side_effects` | `&str` | No | `"None"`、`"ReadOnly"`、`"Writes"`、`"Network"`、`"Destructive"` のいずれか。省略時は `"None"`。 |
| `version` | `&str` | No | セムバー文字列。省略時は `"0.1.0"`。 |
| `keywords_primary` | `&str` | No | カンマ区切りのプライマリキーワード。 |
| `keywords_secondary` | `&str` | No | カンマ区切りのセカンダリキーワード。 |
| `keywords_domain` | `&str` | No | カンマ区切りのドメインタグ。 |
| `keywords_negative` | `&str` | No | カンマ区切りのネガティブキーワード。 |
| `examples` | `&str` | No | JSON リテラル（呼び出し例の配列）。 |
| `caveats` | `&str` | No | パイプ区切り（`\|`）の注意事項リスト。 |
| `preconditions` | `&str` | No | パイプ区切りの前提条件リスト。 |
| `related` | `&str` | No | カンマ区切りの関連ツール名リスト。 |

### フィールドアトリビュート — `#[arg(...)]` / `#[tool(...)]`

個々の**ストラクトフィールド**に配置します：

| アトリビュート | 説明 |
|---|---|
| `#[arg(description = "…")]` | このフィールドの JSON スキーマの `description` を設定します。`///` ドキュメントコメントと同等。 |
| `#[arg(hidden)]` または `#[arg(internal)]` | フィールドを JSON スキーマ（および LLM のビュー）から除外します。 |
| `#[arg(enum_values = "a, b, c")]` | JSON スキーマの `enum` 制約を設定します。 |
| `#[arg(default = "value")]` | JSON スキーマの `default` を設定します。 |
| `#[arg(minimum = N)]` / `#[arg(maximum = N)]` | 数値範囲の制約。 |
| `#[arg(min_length = N)]` / `#[arg(max_length = N)]` | 文字列長の制約。 |
| `#[arg(min_items = N)]` / `#[arg(max_items = N)]` | 配列長の制約。 |
| `#[tool(skip)]` | フィールドを JSON スキーマ**および**デシリアライゼーションから除外します。`run()` が呼ばれる前に `self` からデシリアライズされた args にコピーされます。インジェクトされたコンテキスト（例：`Arc<SharedContext>`）に使用します。 |

> [!IMPORTANT]
> `#[tool(skip)]` でマークされたフィールドには、`serde` がLLM の引数 JSON からデシリアライズしようとしないよう、`#[serde(skip, default)]` も付与する必要があります。

---

## `#[derive(ToolAction)]`

`#[derive(ToolSpec)]` に加えて、完全な `impl ToolAction` を生成します。実行ロジックは通常の `impl` ブロックに **`async fn run(&self)`** メソッドとして記述します。

生成される `execute()` メソッドは以下を行います：
1. JSON の `arguments` 文字列を `Self` にデシリアライズする。
2. レシーバーから `#[tool(skip)]` フィールドをデシリアライズされたインスタンスにコピーする。
3. 生成されたインスタンスで `self.run().await` を呼び出す。
4. `Result<String, ToolError>` を返す。

### 必要な derive

ストラクトには以下の derive も必要です：

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
```

- `Deserialize` — 上記ステップ 1 に使用。
- `JsonSchema` — `parameters` スキーマの生成に使用。

### 完全な使用例

```rust
use std::sync::Arc;
use ene_tool_common::prelude::*;

/// ツールサーバーによってインジェクトされる共有状態。
pub struct SharedContext {
    pub base_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "my_ns",
    name = "do_thing",
    display_name = "Do Thing",
    summary = "指定されたパスに対して処理を実行します。",
    description = "`path` のファイルを読み込んでその内容を返します。\
                   テキストファイルのみ対応しています。",
    category = "Utility",
    side_effects = "ReadOnly",
    keywords_primary = "thing, do, operate",
    keywords_domain = "filesystem",
    caveats = "バイナリファイルは非対応 | 最大 1 MB",
)]
pub struct DoThingAction {
    /// 読み込むファイルへのパス。絶対パスである必要があります。
    pub path: String,

    /// 返す最大バイト数。デフォルトは 65536。
    #[arg(default = "65536", minimum = 1, maximum = 1048576)]
    pub max_bytes: Option<u64>,

    // ツールサーバーによってインジェクトされます — LLM からは見えません。
    #[tool(skip)]
    #[serde(skip, default)]
    pub context: Arc<Option<SharedContext>>,
}

impl DoThingAction {
    async fn run(&self) -> Result<String, ToolError> {
        let limit = self.max_bytes.unwrap_or(65_536);
        let content = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|e| ToolError::IoError { message: e.to_string() })?;

        let output = content.truncate_chars(limit as usize);
        Ok(output.content.to_string())
    }
}
```

### バイナリへの組み込み

`run_tool_server` は**ジェネリックではありません** — `run_tool_server::<T>()` ではなく、ボックス化された `dyn ToolProvider` を受け取ります。1つ以上の `ToolAction` を `ToolProvider` でラップし（完全な例は [`ene-tool-common`](./ene-tool-common.md#ツールバイナリの組み込み方) を参照）、そのプロバイダーを渡します：

```rust,no_run
// tools/my_tool/src/main.rs
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec, run_tool_server};

mod actions;
use actions::DoThingAction;

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
    let action = DoThingAction { path: String::new(), max_bytes: None, context: Default::default() };
    let provider = MyToolProvider { actions: vec![Box::new(action)] };
    run_tool_server(Box::new(provider)).await?;
    Ok(())
}
```

---

## `#[tool_action(args = T)]`

`#[derive(ToolAction)]` が使用できないケース（たとえばトレイトレベルで `execute()` を `async` にする必要がある場合や、所有していない型にトレイトを実装する場合）向けのアトリビュートマクロです。

```rust
#[tool_action(args = DoThingAction)]
impl ToolAction for DoThingWrapper {
    fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        // マクロが `name()` と `definition()` を自動的に補完します。
        // 実装者は `execute()` のみを記述します。
        todo!()
    }
}
```

マクロは `DoThingAction::TOOL_NAME` と `DoThingAction::spec()`（`ToolSpecArgs` から）を読み取り、対応する `name()` と `definition()` のボディを生成します。

---

## 生成されるスキーマについての補足

- ルート JSON スキーマは常に `{ "type": "object", "additionalProperties": false, … }` になります。
- フィールドの Rust ドキュメントコメント（`///`）は、`#[arg(description = "…")]` がない場合に JSON スキーマの `description` として使用されます。
- `Option<T>` フィールドは `required` に含まれないプロパティを持つスキーマを生成します。
- `Vec<T>` フィールドは `{ "type": "array", "items": … }` を生成します。

---

## 関連ページ

- [`ene-tool-common`](ene-tool-common.md) — `ToolAction` と `ToolSpecArgs` トレイトの定義
- [`ene-tool-proto`](ene-tool-proto.md) — `ToolSpec` の構造
- [ツールの作成方法](../tools/sdk.md) — エンドツーエンドのチュートリアル
