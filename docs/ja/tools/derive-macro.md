# Derive マクロリファレンス

`ene-tool-derive` クレートは、最小限のボイラープレートでツールを構築するための2つの proc マクロを提供します。

## `#[derive(ToolAction)]` (推奨)

すべてを生成する単一の derive マクロ: `ToolSpec` メタデータ、JSON Schema、そして完全な `ToolAction` impl。ビジネスロジックは `async fn run(&self)` に記述します。

### 基本的な例

```rust
use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "greeter",
    name = "hello",
    summary = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greeting, hello",
    side_effects = "ReadOnly"
)]
pub struct HelloAction {
    /// Name to greet.
    name: String,
}

impl HelloAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("Hello, {}!", self.name))
    }
}
```

これが生成するもの:
- `HelloAction::TOOL_NAME` = `"greeter.hello"` (const `&str`)
- `HelloAction::DISPLAY_NAME` = `"Hello"` (const `&str`)
- `HelloAction::SUMMARY` = `"Returns a greeting for the given name."` (const `&str`)
- `HelloAction::spec()` → 自動生成 JSON Schema 付きの完全な `ToolSpec`
- `impl ToolAction for HelloAction` の `name()`, `definition()`, `execute()`
- `execute()` は JSON を `Self` にデシリアライズし、`self.run().await` を呼び出す

### `#[tool(skip)]` によるステートフルアクション

`#[tool(skip)]` でマークされたフィールドは JSON Schema から隠蔽され、ユーザー入力からデシリアライズされず、`execute()` 中に `self` からコピーされます。サンドボックス、データベース、HTTP クライアント、セッションストア等の注入依存に使用します。

```rust
use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<BrowserSessionStore> {
    Arc::new(BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "click",
    summary = "Clicks a page element matching the selector.",
    category = "Browser",
    keywords_primary = "click, element"
)]
pub struct ClickAction {
    /// CSS selector for the element to click.
    selector: String,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<BrowserSessionStore>,
}

impl ClickAction {
    pub fn new(store: Arc<BrowserSessionStore>) -> Self {
        Self {
            selector: String::new(),
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        // self.store がここで利用可能
        Ok(format!("Clicked {}", self.selector))
    }
}
```

**`#[tool(skip)]` フィールドのルール:**
1. `#[tool(skip)]` と一緒に `#[serde(skip, default = "fn_name")]` を追加
2. デフォルト関数はフィールドと同じ型を返す必要がある
3. フィールドは `execute()` 内でプロバイダのインスタンスからデシリアライズされた引数へクローンされる

### プロバイダとの統合

```rust
struct MyProvider {
    store: Arc<BrowserSessionStore>,
}

impl ToolProvider for MyProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![ClickAction::new(self.store.clone()).definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            ClickAction::TOOL_NAME => {
                let action = ClickAction::new(self.store.clone());
                action.execute(arguments).await
            }
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }
}
```

## `#[derive(ToolSpec)]` (低レベル)

`ToolSpec` メタデータと JSON Schema のみを生成 — `ToolAction` impl は生成しません。spec は必要だが `execute()` を手動で記述したい場合に使用します。

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddArgs {
    pub a: f64,
    pub b: f64,
}
```

## 必須 `#[tool(...)]` 属性

| 属性 | 型 | 説明 |
|------|-----|------|
| `name` | 文字列 | 短いアクション名 (例: `"read"`)、または `namespace` なしの場合完全名 |
| `summary` | 文字列 | プライマリ埋め込みフィールドとして使用される1行の要約 |
| `category` | 識別子 | `ToolCategory` バリアント: `Filesystem`, `Shell`, `Browser`, `App`, `WebSearch`, `WebFetch`, `Utility`, `Memory`, `Search`, `Meta` |

## オプション `#[tool(...)]` 属性

| 属性 | 型 | デフォルト | 説明 |
|------|-----|----------|------|
| `namespace` | 文字列 | — | 名前空間プレフィックス。完全名 = `"{namespace}.{name}"` |
| `display_name` | 文字列 | 名前のタイトルケース | 人間向けの表示名 |
| `description` | 文字列 | summary と同じ | 完全なマークダウン説明 |
| `version` | 文字列 | `"1.0.0"` | セマンティックバージョン |
| `side_effects` | 識別子 | `ReadOnly` | `ReadOnly`, `Destructive`, `Idempotent`, または修飾パス |

## キーワード属性

カンマ区切り文字列:

| 属性 | 重み | 説明 |
|------|------|------|
| `keywords_primary` | 1.0 | Tool RAG 用の高重み用語 |
| `keywords_secondary` | 0.6 | 中重み用語 |
| `keywords_domain` | 0.3 | ドメインタグ (言語、フレームワーク、プラットフォーム) |
| `keywords_negative` | -0.5 | ネガティブ用語 — クエリに存在する場合ペナルティ |

## メタデータ属性

カンマ区切り文字列:

| 属性 | 説明 |
|------|------|
| `caveats` | LLM が認識すべき注意点 |
| `preconditions` | 呼び出し前に満たすべき前提条件 |
| `related` | 関連・補完ツールの名前 |

## 例属性

セミコロン区切りリスト、各エントリ: `説明|入力|オプション出力`。

```rust
#[tool(
    examples = "Add 2 and 3|{ \"a\": 2, \"b\": 3 }|2 + 3 = 5; Add 0 and 0|{ \"a\": 0, \"b\": 0 }"
)]
```

## フィールドごとの `#[arg(...)]` 属性

| 属性 | 説明 |
|------|------|
| `internal` / `hidden` | JSON Schema の properties と required からフィールドを削除 |
| `skip` | `internal` のエイリアス (フィールド上の `#[tool(skip)]` と同じ) |
| `enum_values = "a, b, c"` | スキーマに `enum` 制約を追加 |
| `default = "value"` | スキーマに `default` を追加 (数値、bool、文字列) |
| `minimum = 0` / `maximum = 100` | 数値制約 |
| `min_length` / `max_length` | 文字列長制約 |
| `min_items` / `max_items` | 配列長制約 |
| `description = "..."` | doc コメントの説明を上書き |

## JSON Schema 生成

derive マクロは `schemars` を使用して JSON Schema を自動生成します。ヒント:

- 数値には `f64`、文字列には `String`、真偽値には `bool`、整数には `i64`/`u64` を使用
- オプションパラメータには `Option<T>` を使用
- フィールドに `///` doc コメントを追加 — スキーマの `description` になる
- `#[serde(rename = "...")]` は JSON プロパティキーを変更
- `#[serde(alias = "...")]` の値は説明に "Aliases: ..." として追加
- `additionalProperties: false` は常にルートオブジェクトに設定

## 依存関係

```toml
[dependencies]
ene-tool-common = { path = "../common" }
ene-tool-proto = { path = "../../crates/ene-tool-proto" }
ene-tool-derive = { path = "../../crates/ene-tool-derive" }
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
```
