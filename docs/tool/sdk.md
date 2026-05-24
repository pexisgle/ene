# サードパーティツール開発ガイド

`ene-tool-proto` は ene のツールシステム用軽量SDKです。このクレートを使うと、誰でも簡単に ene 用のカスタムツールバイナリを作成できます。

## クイックスタート

### 1. プロジェクトを作る

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

### 2. ToolProvider を実装する

```rust
// src/main.rs
use ene_tool_proto::{
    ToolProvider, ToolDefinition, ToolCategory, ToolError, SandboxConfigData,
    run_tool_server,
};
use async_trait::async_trait;

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "hello".into(),
            description: "ユーザーに挨拶を返す".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "挨拶する相手の名前"
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

    fn set_session_id(&self, _session_id: &str) {
        // 必要に応じてセッションIDを保存する
    }
}
```

### 3. サーバーを起動する

```rust
#[tokio::main]
async fn main() {
    run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

### 4. ビルドして配置する

```bash
cargo build --release
```

バイナリを配置:

```bash
mkdir -p ~/.local/share/dev.pexisgle.ene/tools
cp target/release/my-cool-tool ~/.local/share/dev.pexisgle.ene/tools/
```

### 5. ene に認識させる

`settings.json` の `tools.tools` にツール名を追加:

```json
{
  "tools": {
    "tools": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true },
      "browser": { "enable": true },
      "my-cool-tool": { "enable": true }
    }
  }
}
```

これで完了です。次回 ene 起動時に自動的に `my-cool-tool` プロセスが起動し、LLM がツールを使えるようになります。

## アーキテクチャ概要

```
LLM ◄──→ ene-core ◄── IPC (UDS) ──→ あなたのツールバイナリ
```

各ツールバイナリは独立したプロセスとして動作し、Unix Domain Socket 経由で JSON をやり取りします。
`ene-tool-proto` がこの通信を完全に隠蔽するので、あなたは `ToolProvider` trait の実装だけに集中できます。

## ToolProvider trait

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_tools(&self) -> Vec<ToolDefinition>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    fn set_session_id(&self, session_id: &str);
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}  // デフォルト no-op
}
```

| メソッド | 必須 | 説明 |
|----------|------|------|
| `list_tools()` | はい | 提供するツール定義の一覧を返す（起動時/再接続時） |
| `call_tool()` | はい | ツールを実行する。`name` でツールを判別し、`arguments` (JSON) をパースして処理する |
| `set_session_id()` | はい | セッションIDを受け取る。Undo 等のステート管理に使う |
| `set_sandbox()` | いいえ | サンドボックス設定を受け取る。ファイル操作系ツールでのみ使用 |

## 主要な型

### ToolDefinition

```rust
pub struct ToolDefinition {
    pub name: String,              // ツール名（ユニーク）
    pub description: String,       // LLMに伝える説明文
    pub parameters: serde_json::Value,  // JSON Schema
    pub category: Option<ToolCategory>, // 分類（RAG絞り込みに使用）
    pub keywords: Vec<String>,     // 検索用キーワード
}
```

### ToolCategory

```rust
pub enum ToolCategory {
    Filesystem,  // ファイル操作
    Shell,       // シェル実行
    Browser,     // ブラウザ自動化
    App,         // GUI自動化
    WebSearch,   // Web検索
    Utility,     // ユーティリティ
}
```

カテゴリは LLM がツールを選択する際のヒントになります。適切なカテゴリを設定してください。

### ToolError

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
    IpcTransport { message: String },  // 通常は使わない
}
```

適切なエラー型を返すことで、LLM がエラーを理解してリカバリー行動を取れるようになります。

### SandboxConfigData

```rust
pub struct SandboxConfigData {
    pub enabled: bool,
    pub allowed_directories: Vec<String>,
    pub writable_directories: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub shell_timeout_ms: u64,
    pub max_shell_output_bytes: usize,
    pub max_shell_output_lines: usize,
    pub undo_db_path: Option<String>,
}
```

ファイル操作系ツールでは、この設定を参照してアクセス制御を実装してください。その他のツールでは無視して構いません。

## サンプル: ステートフルなツール

会話のコンテキストを保持するツールの例:

```rust
use std::sync::Mutex;

struct CounterTool {
    count: Mutex<i64>,
}

#[async_trait]
impl ToolProvider for CounterTool {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "increment".into(),
                description: "カウンターを1増やす".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
                category: Some(ToolCategory::Utility),
                keywords: vec!["counter".into()],
            },
            ToolDefinition {
                name: "get_count".into(),
                description: "現在のカウンター値を返す".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
                category: Some(ToolCategory::Utility),
                keywords: vec!["counter".into()],
            },
        ]
    }

    async fn call_tool(&self, name: &str, _arguments: &str) -> Result<String, ToolError> {
        match name {
            "increment" => {
                let mut count = self.count.lock().unwrap();
                *count += 1;
                Ok(format!("Count is now {}", *count))
            }
            "get_count" => {
                let count = self.count.lock().unwrap();
                Ok(format!("Current count: {}", *count))
            }
            _ => Err(ToolError::NotFound { tool_name: name.into() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

## HostRegistry による複数 Provider の統合

1つのバイナリで複数カテゴリのツールを提供したい場合、`HostRegistry` を使うと複数の `ToolProvider` を統合できます:

```rust
use ene_tool_proto::{HostRegistry, run_tool_server};

let mut registry = HostRegistry::new();
registry.add_provider(Box::new(MyToolProvider));
registry.add_provider(Box::new(AnotherToolProvider));

run_tool_server(Box::new(registry)).await.unwrap();
```

ツール名が重複した場合、最初に登録された Provider が優先されます。

## バイナリ配置ルール

| ディレクトリ | 用途 | 例 |
|-------------|------|-----|
| `<exe_dir>/tools/` | ビルドインツール（ene本体に同梱） | `/usr/bin/tools/ene-tools-fs` |
| `~/.local/share/dev.pexisgle.ene/tools/` | ユーザー追加ツール | `~/.local/share/dev.pexisgle.ene/tools/my-cool-tool` |

設定の `tools.tools` にバイナリ名（拡張子なし）を追加すると、`ToolHostManager` が起動時に自動的に発見・起動します。

## ヒント

- **ツール名はユニークに**: ビルトインツールと名前が衝突すると、ビルトインが優先されます。固有のプレフィックスを推奨します
- **description は丁寧に**: LLM がツールを選ぶ判断材料になります。何ができて、いつ使うべきかを明確に書きましょう
- **JSON Schema は正確に**: `required` と `description` を必ず設定してください。LLM が正しい引数を生成するのに役立ちます
- **keywords を活用**: 類似ツールが多い場合、適切なキーワードを設定すると RAG 検索での発見率が上がります
- **引数は JSON 文字列**: `call_tool` の `arguments` は JSON 文字列です。`serde_json::from_str` でパースしてください
- **エラーは具体的に**: 詳細なエラーメッセージを返すと、LLM がリカバリー行動を取りやすくなります
