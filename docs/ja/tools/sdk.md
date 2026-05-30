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
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### 2. ToolProvider を実装

```rust
// src/main.rs
use ene_tool_proto::{
    ToolProvider, ToolDefinition, ToolCategory, ToolError,
    SandboxConfigData, run_tool_server,
};
use async_trait::async_trait;

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "hello".into(),
            description: "Returns a greeting for the given name.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name to greet"
                    }
                },
                "required": ["name"]
            }),
            category: Some(ToolCategory::Utility),
            keywords: vec!["greeting".into(), "hello".into()],
        }]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "hello" => {
                let args: serde_json::Value = serde_json::from_str(arguments)
                    .map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                let name = args["name"].as_str().unwrap_or("world");
                Ok(format!("Hello, {}!", name))
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

### 3. サーバーを起動

```rust
#[tokio::main]
async fn main() {
    run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

### 4. ビルドして配置

```bash
cargo build --release
mkdir -p ~/.local/share/dev.pexisgle.ene/tools
cp target/release/my-cool-tool ~/.local/share/dev.pexisgle.ene/tools/
```

### 5. 設定で有効化

```json
{
  "tools": {
    "tools": {
      "my-cool-tool": { "enable": true }
    }
  }
}
```

## ToolProvider トレイトリファレンス

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// このプロバイダが公開するツール一覧を返す
    fn list_tools(&self) -> Vec<ToolDefinition>;

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

## ToolDefinition

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,  // パラメータの JSON Schema
    pub category: Option<ToolCategory>,
    pub keywords: Vec<String>,
}
```

### ToolCategory

| バリアント | 使用場面 |
|-----------|---------|
| `Filesystem` | ファイル読み取り、書き込み、編集操作 |
| `Shell` | シェルコマンド実行 |
| `Browser` | Web フェッチ、ブラウザ自動化 |
| `WebSearch` | 検索エンジンクエリ |
| `App` | GUI 自動化、デスクトップ操作 |
| `Utility` | ヘルパーツール (時刻、システム情報など) |

## ToolError

`ene-tool-proto` の `ToolError` 型 (ツールバイナリが使用):

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
}
```

**注意:** `ene-tool-host` は独自の `ToolError` 型を持ち、ドメイン固有のバリアント (`FileNotFound`, `FileTooLarge`, `CommandBlocked`, `ShellTimeout`, `BrowserError`, `AppError`, `IpcClient` など) が追加されています。ホスト側のエラーは IPC 境界でプロトコル側のエラーにマッピングされます。

## IPC ライフサイクル

```
ツールバイナリ起動
  → ENE_TOOL_SOCKET で待機 (ToolHostManager が環境変数として提供)
  → IpcRequest::Initialize を受信
  → ツールがサンドボックス + 設定で初期化
  → CallTool リクエストを処理可能に
```

## ベストプラクティス

1. **説明は LLM に優しく** — LLM がツールをいつ呼び出すかを説明が決定します。いつ、どのように使うかを明確に記述してください。
2. **JSON Schema を適切に** — パラメータは LLM の関数呼び出しによって検証されます。`required`, `type`, `description`, `enum` 制約を活用してください。
3. **キーワードが重要** — キーワードは Tool RAG 埋め込みに使用されます。同義語や関連用語を含めてください。
4. **エラーを適切に処理** — 明確なメッセージで `ToolError` バリアントを返してください。LLM はエラーメッセージに基づいて使用法を修正しようとすることがあります。
5. **セッション分離** — ツールがセッション状態を保持する場合、`set_session_id()` を使用してスコープしてください。
6. **設定スキーマ** — 設定可能なパラメータがある場合は `config_schema()` を実装してください。自動生成される `settings.schema.json` に反映されます。
