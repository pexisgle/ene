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
ene-tool-proto = { git = "https://github.com/pexisgle/Ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/Ene" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
```

### 2. `#[derive(ToolSpec)]` で引数構造体を定義

各ツールは型付きの引数構造体を持ちます。derive マクロは `schemars` による自動生成 JSON Schema を含む `spec() -> ToolSpec` メソッドと、ディスパッチ用の `TOOL_NAME` 定数を生成します。

```rust
use ene_tool_derive::ToolSpec;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "greeter",
    name = "hello",
    summary = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greeting, hello",
    side_effects = "ReadOnly"
)]
pub struct HelloArgs {
    /// Name to greet.
    pub name: String,
}
```

derive マクロが生成するもの:
- `HelloArgs::TOOL_NAME` = `"greeter.hello"`
- `HelloArgs::spec()` → `schemars` による自動生成 JSON Schema 付きの完全な `ToolSpec`

### 3. ToolProvider を実装

```rust
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec, run_tool_server};
use async_trait::async_trait;

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![HelloArgs::spec()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            HelloArgs::TOOL_NAME => {
                let args: HelloArgs = serde_json::from_str(arguments)
                    .map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                Ok(format!("Hello, {}!", args.name))
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

## 複数ツール

複数の仕様を返すことで、単一プロバイダから複数のツールを公開できます:

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddArgs { pub a: f64, pub b: f64 }

#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "calculator", name = "subtract", summary = "Subtract two numbers.", category = "Utility")]
pub struct SubtractArgs { pub a: f64, pub b: f64 }

struct CalculatorProvider;

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![AddArgs::spec(), SubtractArgs::spec()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            AddArgs::TOOL_NAME => {
                let args: AddArgs = serde_json::from_str(arguments)?;
                Ok(format!("{} + {} = {}", args.a, args.b, args.a + args.b))
            }
            SubtractArgs::TOOL_NAME => {
                let args: SubtractArgs = serde_json::from_str(arguments)?;
                Ok(format!("{} - {} = {}", args.a, args.b, args.a - args.b))
            }
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

各ツールはファーストクラスの `ToolSpec` であり、独自の型付き引数、自動生成 JSON Schema、Tool RAG パイプライン用の豊富なメタデータを持ちます。

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

## `#[derive(ToolSpec)]` 属性

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
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
pub struct AddArgs {
    /// First operand.
    pub a: f64,
    /// Second operand.
    pub b: f64,
}
```

詳細は [derive-macro.md](derive-macro.md) を参照してください。

## ToolName

検証済みの名前空間付きツール識別子:

```rust
ToolName::new("filesystem.read")  // namespace = "filesystem", action = "read"
ToolName::new("get_current_time") // 名前空間なし
```

## KeywordSet

Tool RAG 用の重み付き階層キーワード:

```rust
KeywordSet {
    primary: vec!["read", "open", "cat"],      // 重み 1.0
    secondary: vec!["file", "filesystem"],      // 重み 0.6
    domain: vec!["linux", "posix"],             // 重み 0.3
    negative: vec!["write", "delete"],          // 重み -0.5 (ペナルティ)
}
```

## SideEffects

```rust
pub enum SideEffects {
    ReadOnly,                           // 副作用なし
    FileSystem { mutates: bool },       // ファイル I/O
    Network { external: bool },         // ネットワークアクセス
    System { privileged: bool },        // プロセス生成、シグナル
    Browser { mutates_dom: bool },      // ブラウザ自動化
    Destructive,                        // データ損失の可能性
    Idempotent,                         // 安全にリトライ可能
}
```

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
pub type ToolError = EneToolProtoError;

pub enum EneToolProtoError {
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

1. **ツールごとに 1 つの引数構造体** — 各ツールに独自の `#[derive(ToolSpec)]` 構造体を持たせます。型付き引数、自動生成 JSON Schema、ディスパッチ用 `TOOL_NAME` 定数が得られます。
2. **名前空間を使用** — 関連ツールを名前空間でグループ化 (例: `calculator.add`, `calculator.subtract`)。
3. **良い要約を書く** — 要約は Tool RAG のプライマリ埋め込みフィールドです。いつ、どのように使うかを明確に記述してください。
4. **キーワードを活用** — 同義語や関連用語を含めてください。プライマリキーワードは RAG スコアリングで最も高く重み付けされます。
5. **副作用を正しく設定** — LLM がツールの安全性を理解し、サンドボックスが正しい判断を行うのに役立ちます。
6. **エラーを適切に処理** — 明確なメッセージで `ToolError` バリアントを返してください。LLM はエラーメッセージに基づいて使用法を修正しようとすることがあります。