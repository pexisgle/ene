# SDK: カスタムツールの作成

`ene-tool-proto` は ene に統合するカスタムツールバイナリを作成するための軽量 SDK です。

## クイックスタート

### 1. プロジェクトを作成

```toml
# Cargo.toml
[package]
name = "my-cool-tool"
version = "0.1.0"
edition = "2024"

[dependencies]
ene-tool-common = { git = "https://github.com/pexisgle/Ene" }
ene-tool-proto = { git = "https://github.com/pexisgle/Ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/Ene" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
async-trait = "0.1"
```

### 2. `#[derive(ToolAction)]` でアクションを定義

各ツールは `#[derive(ToolAction)]` を付けた構造体です。derive マクロが `ToolSpec`、JSON Schema、`ToolAction` impl を生成します。ビジネスロジックは `async fn run(&self)` に記述します。

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

### 3. ToolProvider を実装

```rust
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![HelloAction::default().definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            HelloAction::TOOL_NAME => {
                let action = HelloAction::default();
                action.execute(arguments).await
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

### 4. サーバーを起動

```rust
#[tokio::main]
async fn main() {
    run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

### 5. ビルドして配置

```bash
cargo build --release
mkdir -p ~/.local/share/dev.pexisgle.ene/tools
cp target/release/my-cool-tool ~/.local/share/dev.pexisgle.ene/tools/
```

### 6. 設定で有効化

```json
{
  "tools": {
    "tools": {
      "my-cool-tool": { "enable": true }
    }
  }
}
```

## ステートフルアクション

サンドボックス、データベース、HTTP クライアント等の依存注入が必要なアクションには `#[tool(skip)]` を使用します:

```rust
use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_client() -> reqwest::Client {
    reqwest::Client::new()
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "myapi",
    name = "fetch",
    summary = "Fetch data from the API.",
    category = "WebFetch"
)]
pub struct FetchAction {
    /// The endpoint to call.
    endpoint: String,

    #[tool(skip)]
    #[serde(skip, default = "default_client")]
    client: reqwest::Client,
}

impl FetchAction {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            endpoint: String::new(),
            client,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let resp = self.client.get(&self.endpoint).send().await
            .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
        Ok(resp.text().await.unwrap_or_default())
    }
}
```

プロバイダは実際の依存でアクションを構築し、`execute()` がそれらをコピーします:

```rust
async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
    match name {
        FetchAction::TOOL_NAME => {
            let action = FetchAction::new(self.client.clone());
            action.execute(arguments).await
        }
        _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
    }
}
```

## 複数ツール

単一プロバイダから複数のツールを公開できます:

```rust
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddAction { a: f64, b: f64 }

impl AddAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("{} + {} = {}", self.a, self.b, self.a + self.b))
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "calculator", name = "subtract", summary = "Subtract two numbers.", category = "Utility")]
pub struct SubtractAction { a: f64, b: f64 }

impl SubtractAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("{} - {} = {}", self.a, self.b, self.a - self.b))
    }
}

struct CalculatorProvider;

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![
            AddAction::default().definition(),
            SubtractAction::default().definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            AddAction::TOOL_NAME => AddAction::default().execute(arguments).await,
            SubtractAction::TOOL_NAME => SubtractAction::default().execute(arguments).await,
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

## ToolProvider トレイトリファレンス

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// このプロバイダが公開するツール一覧を返す
    fn list_specs(&self) -> Vec<ToolSpec>;

    /// メガツール用のアクションごとのメタデータを返す (デフォルト: 空)
    fn list_action_specs(&self) -> Vec<ActionSpec> { vec![] }

    /// ツール名と JSON 引数でツールを実行
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// セッション ID が変更されたときに呼び出される (セッション状態用)
    fn set_session_id(&self, session_id: &str);

    /// サンドボックス設定を受信 (ファイルシステムツール用)
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// ペンディング中の破壊的操作の権限リクエストを承認
    fn approve_permission(&self, _request_id: &str) {}

    /// セッション全体の権限許可パターンを追加 (アクション + ターゲットグロブ)
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// settings.json からツール固有設定を受信
    fn set_config(&self, _config: &serde_json::Value) {}

    /// このツールが受け付ける設定の JSON Schema を返す
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

## ToolSpec

LLM 向けの構造化されたツール仕様:

```rust
pub struct ToolSpec {
    pub name: ToolName,           // 例: "filesystem.read"
    pub version: ToolVersion,     // セマンティックバージョン (1.0.0)
    pub display_name: String,     // "Read File"
    pub summary: String,          // 1行 (埋め込みに使用)
    pub description: String,      // 完全なマークダウン
    pub category: ToolCategory,   // Filesystem, Utility 等
    pub keywords: KeywordSet,     // 構造化キーワードバッグ
    pub parameters: serde_json::Value,  // JSON Schema (schemars による自動生成)
    pub examples: Vec<ToolExample>,
    pub caveats: Vec<String>,
    pub side_effects: SideEffects,
    pub preconditions: Vec<String>,
    pub related: Vec<ToolName>,
}
```

## `#[tool(...)]` 属性

```rust
#[derive(ToolAction, JsonSchema, Deserialize)]
#[tool(
    // 必須
    namespace = "calculator",        // 名前空間プレフィックス
    name = "add",                    // アクション名 (完全名: "calculator.add")
    summary = "Add two numbers.",    // 1行の要約 (埋め込みフィールド)
    category = "Utility",            // ToolCategory バリアント

    // オプション
    display_name = "Add Numbers",    // デフォルト: 名前のタイトルケース
    description = "Longer markdown", // デフォルト: summary
    version = "1.0.0",               // デフォルト: 1.0.0
    side_effects = "ReadOnly",       // デフォルト: ReadOnly

    // キーワード (カンマ区切り)
    keywords_primary = "add, sum, plus",
    keywords_secondary = "math, number",
    keywords_domain = "arithmetic",
    keywords_negative = "subtract, remove",

    // メタデータ (カンマ区切り)
    caveats = "Division by zero returns an error.",
    preconditions = "Arguments must be valid numbers.",
    related = "calculator.subtract, calculator.multiply",

    // 例 (セミコロン区切り、各: 説明|入力|出力)
    examples = "Add 2 and 3|{ \"a\": 2, \"b\": 3 }|2 + 3 = 5"
)]
pub struct AddAction {
    /// First operand.
    a: f64,
    /// Second operand.
    b: f64,
}
```

詳細は [derive-macro.md](derive-macro.md) を参照してください。

## ToolCategory

| バリアント | 使用場面 |
|-----------|---------|
| `Filesystem` | ファイル読み取り、書き込み、編集操作 |
| `Shell` | シェルコマンド実行 |
| `Browser` | Web フェッチ、ブラウザ自動化 |
| `App` | GUI 自動化、デスクトップ操作 |
| `WebSearch` | 検索エンジンクエリ |
| `WebFetch` | URL フェッチ |
| `Utility` | ヘルパーツール (時刻、システム情報など) |
| `Memory` | 長期記憶操作 |
| `Search` | ローカル検索 / ユーザードキュメントの RAG |
| `Meta` | 自己分析、ツール選択 |

## ToolError

```rust
pub enum ToolError {
    NotFound { tool_name: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    IoError { message: String },
    Timeout { message: String },
    Internal { message: String },
    IpcTransport { message: String },
    PermissionRequired { request_id: String, action: String, target: String, description: String },
    UserInputRequired { request_id: String, prompt: UserInputPrompt },
    FileNotFound { path: String },
    FileTooLarge { path: String, size: u64, limit: u64 },
    CommandBlocked { command: String, reason: String },
    ShellTimeout { command: String, timeout_ms: u64 },
    ShellOutputTooLarge { size: u64, limit: u64 },
    BrowserError { message: String },
    AppError { message: String },
    WebSearchError { message: String },
    IpcClient { message: String },
    Other { message: String },
}
```

## IPC ライフサイクル

```
ツールバイナリ起動
  → ENE_TOOL_SOCKET で待機 (ToolHostManager が環境変数として提供)
  → IpcRequest::Handshake を受信 → HandshakeAck を応答
  → IpcRequest::Initialize を受信
  → ツールがサンドボックス + 設定で初期化
  → CallTool リクエストを処理可能に
```

## ベストプラクティス

1. **`#[derive(ToolAction)]` を使用** — 一つの derive で spec、schema、ディスパッチを生成。ロジックは `async fn run(&self)` に記述。
2. **依存には `#[tool(skip)]` を使用** — サンドボックス、データベース、HTTP クライアント等は LLM から隠蔽され、プロバイダから注入されます。
3. **名前空間を使用** — 関連ツールを名前空間でグループ化 (例: `calculator.add`, `calculator.subtract`)。
4. **良い要約を書く** — 要約は Tool RAG のプライマリ埋め込みフィールドです。いつ、どのように使うかを明確に記述してください。
5. **キーワードを活用** — 同義語や関連用語を含めてください。プライマリキーワードは RAG スコアリングで最も高く重み付けされます。
6. **副作用を正しく設定** — LLM がツールの安全性を理解し、サンドボックスが正しい判断を行うのに役立ちます。
7. **エラーを適切に処理** — 明確なメッセージで `ToolError` バリアントを返してください。LLM はエラーメッセージに基づいて使用法を修正しようとすることがあります。
